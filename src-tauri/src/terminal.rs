//! In-memory, independently owned SSH PTY sessions. Page navigation never owns
//! a channel, and terminal contents are deliberately never written to data/.
use crate::ssh::Connection;
use base64::{engine::general_purpose::STANDARD, Engine};
use russh::{client, Channel, ChannelMsg, ChannelWriteHalf};
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
    time::timeout,
};
use tokio_util::sync::CancellationToken;

const OUTPUT_LIMIT: usize = 2 * 1024 * 1024;
const READ_LIMIT: usize = 64 * 1024;
const INPUT_LIMIT: usize = 64 * 1024;
const MAX_PER_PROFILE: usize = 16;
const MAX_TOTAL: usize = 64;
const MAX_CURSOR: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSession {
    pub id: String,
    pub profile_id: String,
    pub title: String,
    pub status: String,
    pub exit_code: Option<u32>,
    pub error: Option<String>,
    pub cols: u32,
    pub rows: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalRead {
    data: String,
    cursor: u64,
    start_cursor: u64,
    session: TerminalSession,
}

#[derive(Default)]
struct Output {
    bytes: VecDeque<u8>,
    end: u64,
}

impl Output {
    fn append(&mut self, data: &[u8]) {
        self.end = self.end.saturating_add(data.len() as u64);
        if data.len() >= OUTPUT_LIMIT {
            self.bytes.clear();
            self.bytes.extend(&data[data.len() - OUTPUT_LIMIT..]);
        } else {
            let excess = self
                .bytes
                .len()
                .saturating_add(data.len())
                .saturating_sub(OUTPUT_LIMIT);
            self.bytes.drain(..excess);
            self.bytes.extend(data);
        }
    }

    fn read(&self, cursor: u64) -> Result<(Vec<u8>, u64, u64), String> {
        if cursor > self.end || cursor > MAX_CURSOR {
            return Err("终端读取位置无效".into());
        }
        let oldest = self.end - self.bytes.len() as u64;
        let start = cursor.max(oldest);
        let data: Vec<_> = self
            .bytes
            .iter()
            .skip((start - oldest) as usize)
            .take(READ_LIMIT)
            .copied()
            .collect();
        let next = start + data.len() as u64;
        Ok((data, next, start))
    }
}

struct State {
    session: TerminalSession,
    output: Output,
}

enum Command {
    Write(Vec<u8>, oneshot::Sender<Result<(), String>>),
    Resize(u32, u32, oneshot::Sender<Result<(), String>>),
}

struct Terminal {
    state: Mutex<State>,
    sequence: u64,
    commands: mpsc::Sender<Command>,
    cancel: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl Terminal {
    fn session(&self) -> TerminalSession {
        self.state.lock().unwrap().session.clone()
    }

    fn output(&self, data: &[u8]) {
        self.state.lock().unwrap().output.append(data);
    }

    fn finish(&self, error: Option<String>) {
        let mut state = self.state.lock().unwrap();
        if state.session.status == "opening" || state.session.status == "running" {
            state.session.status = if error.is_some() { "error" } else { "exited" }.into();
            state.session.error = error;
        }
    }

    async fn command(
        &self,
        command: Command,
        reply: oneshot::Receiver<Result<(), String>>,
    ) -> Result<(), String> {
        if self.session().status != "running" || self.cancel.is_cancelled() {
            return Err("终端已结束，请新建终端".into());
        }
        // Bounded queue plus async waiting applies backpressure to pasted data.
        tokio::select! {
            _ = self.cancel.cancelled() => return Err("终端已关闭".into()),
            sent = self.commands.send(command) => sent.map_err(|_| "终端已结束")?,
        }
        tokio::select! {
            _ = self.cancel.cancelled() => Err("终端已结束".into()),
            result = reply => result.map_err(|_| "终端已结束")?,
        }
    }

    async fn stop(&self) {
        self.cancel.cancel();
        let task = self.task.lock().unwrap().take();
        if let Some(mut task) = task {
            if timeout(Duration::from_secs(3), &mut task).await.is_err() {
                task.abort();
                let _ = task.await;
            }
        }
    }
}

#[derive(Default)]
struct Sessions {
    entries: HashMap<String, Arc<Terminal>>,
    sequence: u64,
    counters: HashMap<String, u64>,
}

pub struct TerminalManager {
    sessions: Mutex<Sessions>,
    cancelled: Mutex<CancellationToken>,
    pending_opens: Arc<Semaphore>,
}

impl TerminalManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(Sessions::default()),
            cancelled: Mutex::new(CancellationToken::new()),
            pending_opens: Arc::new(Semaphore::new(MAX_TOTAL)),
        }
    }

    pub fn list(&self, profile_id: &str) -> Vec<TerminalSession> {
        let sessions = self.sessions.lock().unwrap();
        let mut terminals: Vec<_> = sessions
            .entries
            .values()
            .filter(|terminal| terminal.session().profile_id == profile_id)
            .collect();
        terminals.sort_by_key(|terminal| terminal.sequence);
        terminals
            .into_iter()
            .map(|terminal| terminal.session())
            .collect()
    }

    fn get(&self, profile_id: &str, id: &str) -> Result<Arc<Terminal>, String> {
        self.sessions
            .lock()
            .unwrap()
            .entries
            .get(id)
            .filter(|terminal| terminal.session().profile_id == profile_id)
            .cloned()
            .ok_or_else(|| "终端不存在或已关闭".into())
    }

    pub async fn open(
        &self,
        profile_id: &str,
        connection: Arc<Connection>,
        cols: u32,
        rows: u32,
    ) -> Result<TerminalSession, String> {
        validate_dimensions(cols, rows)?;
        if connection.is_closed() || self.cancelled.lock().unwrap().is_cancelled() {
            return Err("SSH 连接已关闭".into());
        }
        let permit = self
            .pending_opens
            .clone()
            .try_acquire_owned()
            .map_err(|_| "终端通道仍在建立，请稍后重试")?;
        let (sender, receiver) = mpsc::channel(32);
        let (ready, opened) = oneshot::channel();
        let terminal = {
            let mut sessions = self.sessions.lock().unwrap();
            if self.cancelled.lock().unwrap().is_cancelled() {
                return Err("应用正在关闭".into());
            }
            if sessions.entries.len() >= MAX_TOTAL {
                return Err("最多同时保留 64 个终端，请先关闭不用的终端".into());
            }
            if sessions
                .entries
                .values()
                .filter(|terminal| terminal.session().profile_id == profile_id)
                .count()
                >= MAX_PER_PROFILE
            {
                return Err("每台设备最多保留 16 个终端，请先关闭不用的终端".into());
            }
            sessions.sequence += 1;
            let sequence = sessions.sequence;
            let number = sessions.counters.entry(profile_id.into()).or_default();
            *number += 1;
            let terminal = Arc::new(Terminal {
                state: Mutex::new(State {
                    session: TerminalSession {
                        id: uuid::Uuid::new_v4().to_string(),
                        profile_id: profile_id.into(),
                        title: format!("终端 {number}"),
                        status: "opening".into(),
                        exit_code: None,
                        error: None,
                        cols,
                        rows,
                    },
                    output: Output::default(),
                }),
                sequence,
                commands: sender,
                cancel: self.cancelled.lock().unwrap().child_token(),
                task: Mutex::new(None),
            });
            sessions
                .entries
                .insert(terminal.session().id, terminal.clone());
            // Spawn while still under the registry lock, so shutdown cannot
            // remove a reservation before its task has been registered.
            let worker = terminal.clone();
            *terminal.task.lock().unwrap() = Some(tokio::spawn(async move {
                run(worker, connection, permit, receiver, ready).await;
            }));
            terminal
        };
        match opened.await {
            Ok(Ok(())) => Ok(terminal.session()),
            result => {
                let id = terminal.session().id;
                let _ = self.close(profile_id, &id).await;
                Err(match result {
                    Ok(Err(error)) => error,
                    _ => "终端建立已取消".into(),
                })
            }
        }
    }

    pub fn read(&self, profile_id: &str, id: &str, cursor: u64) -> Result<TerminalRead, String> {
        let terminal = self.get(profile_id, id)?;
        let state = terminal.state.lock().unwrap();
        let (data, cursor, start_cursor) = state.output.read(cursor)?;
        Ok(TerminalRead {
            data: STANDARD.encode(data),
            cursor,
            start_cursor,
            session: state.session.clone(),
        })
    }

    pub async fn write(&self, profile_id: &str, id: &str, data: &str) -> Result<(), String> {
        if data.len() > INPUT_LIMIT.div_ceil(3) * 4 {
            return Err("单次终端输入不能超过 64 KiB".into());
        }
        let data = STANDARD.decode(data).map_err(|_| "终端输入格式无效")?;
        if data.len() > INPUT_LIMIT {
            return Err("单次终端输入不能超过 64 KiB".into());
        }
        let terminal = self.get(profile_id, id)?;
        let (sender, receiver) = oneshot::channel();
        terminal
            .command(Command::Write(data, sender), receiver)
            .await
    }

    pub async fn resize(
        &self,
        profile_id: &str,
        id: &str,
        cols: u32,
        rows: u32,
    ) -> Result<(), String> {
        validate_dimensions(cols, rows)?;
        let terminal = self.get(profile_id, id)?;
        let (sender, receiver) = oneshot::channel();
        terminal
            .command(Command::Resize(cols, rows, sender), receiver)
            .await
    }

    pub fn rename(
        &self,
        profile_id: &str,
        id: &str,
        title: &str,
    ) -> Result<TerminalSession, String> {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > 80 || title.chars().any(char::is_control) {
            return Err("终端备注需为 1 到 80 个字符，且不能包含控制字符".into());
        }
        let terminal = self.get(profile_id, id)?;
        let mut state = terminal.state.lock().unwrap();
        state.session.title = title.into();
        Ok(state.session.clone())
    }

    pub async fn close(&self, profile_id: &str, id: &str) -> Result<(), String> {
        let terminal = {
            let mut sessions = self.sessions.lock().unwrap();
            if !sessions
                .entries
                .get(id)
                .is_some_and(|terminal| terminal.session().profile_id == profile_id)
            {
                return Err("终端不存在或已关闭".into());
            }
            sessions.entries.remove(id).unwrap()
        };
        terminal.stop().await;
        Ok(())
    }

    pub async fn remove_profile(&self, profile_id: &str) {
        let terminals = {
            let mut sessions = self.sessions.lock().unwrap();
            let ids: Vec<_> = sessions
                .entries
                .iter()
                .filter(|(_, terminal)| terminal.session().profile_id == profile_id)
                .map(|(id, _)| id.clone())
                .collect();
            sessions.counters.remove(profile_id);
            ids.into_iter()
                .filter_map(|id| sessions.entries.remove(&id))
                .collect::<Vec<_>>()
        };
        stop_all(terminals).await;
    }

    pub async fn shutdown(&self) {
        self.cancelled.lock().unwrap().cancel();
        let terminals = self
            .sessions
            .lock()
            .unwrap()
            .entries
            .drain()
            .map(|(_, terminal)| terminal)
            .collect();
        stop_all(terminals).await;
    }

    pub fn kill(&self) {
        self.cancelled.lock().unwrap().cancel();
        for (_, terminal) in self.sessions.lock().unwrap().entries.drain() {
            terminal.cancel.cancel();
            if let Some(task) = terminal.task.lock().unwrap().take() {
                task.abort();
            }
        }
    }

    pub fn resume(&self) {
        let sessions = self.sessions.lock().unwrap();
        if sessions.entries.is_empty() {
            *self.cancelled.lock().unwrap() = CancellationToken::new();
        }
    }
}

impl Drop for TerminalManager {
    fn drop(&mut self) {
        self.kill();
    }
}

fn validate_dimensions(cols: u32, rows: u32) -> Result<(), String> {
    if !(2..=500).contains(&cols) || !(1..=300).contains(&rows) {
        return Err("终端尺寸无效（列 2–500，行 1–300）".into());
    }
    Ok(())
}

async fn stop_all(terminals: Vec<Arc<Terminal>>) {
    let mut tasks = tokio::task::JoinSet::new();
    for terminal in terminals {
        terminal.cancel.cancel();
        tasks.spawn(async move {
            terminal.stop().await;
        });
    }
    while tasks.join_next().await.is_some() {}
}

async fn await_success(
    channel: &mut Channel<client::Msg>,
    terminal: &Terminal,
    description: &str,
) -> Result<(), String> {
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Success => return Ok(()),
            ChannelMsg::Failure => return Err(format!("服务器拒绝{description}")),
            ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                terminal.output(&data)
            }
            ChannelMsg::Eof | ChannelMsg::Close => {
                return Err(format!("建立{description}时服务器关闭了通道"))
            }
            _ => {}
        }
    }
    Err("SSH 终端通道已关闭".into())
}

async fn run(
    terminal: Arc<Terminal>,
    connection: Arc<Connection>,
    permit: OwnedSemaphorePermit,
    receiver: mpsc::Receiver<Command>,
    ready: oneshot::Sender<Result<(), String>>,
) {
    let mut channel = match connection
        .open_terminal_channel(&terminal.cancel, permit)
        .await
    {
        Ok(channel) => channel,
        Err(error) => {
            terminal.finish(Some(error.clone()));
            terminal.cancel.cancel();
            let _ = ready.send(Err(error));
            return;
        }
    };
    let dimensions = terminal.session();
    let handshake = async {
        channel
            .request_pty(
                true,
                "xterm-256color",
                dimensions.cols,
                dimensions.rows,
                0,
                0,
                &[],
            )
            .await
            .map_err(|error| error.to_string())?;
        await_success(&mut channel, &terminal, "PTY 终端").await?;
        channel
            .request_shell(true)
            .await
            .map_err(|error| error.to_string())?;
        await_success(&mut channel, &terminal, "交互式 Shell").await
    };
    let opened = tokio::select! {
        _ = terminal.cancel.cancelled() => Err("终端已关闭".into()),
        reason = connection.closed() => Err(reason),
        result = timeout(Duration::from_secs(12), handshake) => result.unwrap_or_else(|_| Err("SSH 终端建立超时".into())),
    };
    if let Err(error) = opened {
        terminal.finish(Some(error.clone()));
        terminal.cancel.cancel();
        let _ = ready.send(Err(error));
        let _ = timeout(Duration::from_secs(1), channel.close()).await;
        return;
    }
    terminal.state.lock().unwrap().session.status = "running".into();
    let _ = ready.send(Ok(()));
    let (mut reader, writer) = channel.split();
    let writer_terminal = terminal.clone();
    let writer_connection = connection.clone();
    let mut write_task = tokio::spawn(async move {
        write_loop(writer_terminal, writer_connection, writer, receiver).await;
    });
    loop {
        let message = tokio::select! {
            _ = terminal.cancel.cancelled() => break,
            reason = connection.closed() => { terminal.finish(Some(reason)); break; },
            message = reader.wait() => message,
        };
        match message {
            Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                terminal.output(&data)
            }
            Some(ChannelMsg::ExitStatus { exit_status }) => {
                terminal.state.lock().unwrap().session.exit_code = Some(exit_status)
            }
            Some(ChannelMsg::ExitSignal {
                signal_name,
                error_message,
                ..
            }) => {
                terminal.finish(Some(format!(
                    "终端被信号 {signal_name:?} 结束{}",
                    if error_message.is_empty() {
                        String::new()
                    } else {
                        format!("：{error_message}")
                    }
                )));
            }
            None | Some(ChannelMsg::Close) => {
                terminal.finish(None);
                break;
            }
            _ => {}
        }
    }
    terminal.finish(None);
    terminal.cancel.cancel();
    if timeout(Duration::from_secs(2), &mut write_task)
        .await
        .is_err()
    {
        write_task.abort();
        let _ = write_task.await;
    }
}

async fn write_loop(
    terminal: Arc<Terminal>,
    connection: Arc<Connection>,
    writer: ChannelWriteHalf<client::Msg>,
    mut receiver: mpsc::Receiver<Command>,
) {
    loop {
        let command = tokio::select! {
            _ = terminal.cancel.cancelled() => break,
            _ = connection.closed() => break,
            command = receiver.recv() => match command { Some(command) => command, None => break },
        };
        let operation = async {
            match &command {
                Command::Write(data, _) => writer.data(data.as_slice()).await,
                Command::Resize(cols, rows, _) => writer.window_change(*cols, *rows, 0, 0).await,
            }
            .map_err(|error| format!("终端发送失败：{error}"))
        };
        let result = tokio::select! {
            _ = terminal.cancel.cancelled() => Err("终端已关闭".into()),
            reason = connection.closed() => Err(reason),
            result = timeout(Duration::from_secs(30), operation) => result.unwrap_or_else(|_| Err("终端输入发送超时".into())),
        };
        if let Err(error) = &result {
            terminal.finish(Some(error.clone()));
            terminal.cancel.cancel();
        }
        match command {
            Command::Write(_, reply) => {
                let _ = reply.send(result);
            }
            Command::Resize(cols, rows, reply) => {
                if result.is_ok() {
                    let mut state = terminal.state.lock().unwrap();
                    state.session.cols = cols;
                    state.session.rows = rows;
                }
                let _ = reply.send(result);
            }
        }
    }
    let _ = timeout(Duration::from_secs(1), writer.close()).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_reports_truncation_and_reads_bounded_raw_bytes() {
        let mut output = Output::default();
        output.append(&vec![b'x'; OUTPUT_LIMIT - 2]);
        output.append("中文".as_bytes());
        assert_eq!(output.bytes.len(), OUTPUT_LIMIT);
        let (data, next, start) = output.read(0).unwrap();
        assert_eq!(start, 4);
        assert_eq!(data.len(), READ_LIMIT);
        assert_eq!(next, 4 + READ_LIMIT as u64);
        let (data, _, _) = output.read(output.end - 6).unwrap();
        assert_eq!(data, "中文".as_bytes());
        assert!(output.read(output.end + 1).is_err());
    }

    #[test]
    fn oversized_append_retains_only_latest_bytes() {
        let mut output = Output::default();
        let mut data = vec![b'a'; OUTPUT_LIMIT + 7];
        *data.last_mut().unwrap() = b'z';
        output.append(&data);
        assert_eq!(output.end, data.len() as u64);
        assert_eq!(output.bytes.len(), OUTPUT_LIMIT);
        assert_eq!(output.bytes.back(), Some(&b'z'));
        assert_eq!(output.read(0).unwrap().2, 7);
    }

    #[test]
    fn pty_dimensions_are_bounded() {
        assert!(validate_dimensions(80, 24).is_ok());
        for dimensions in [(0, 24), (80, 0), (501, 24), (80, 301)] {
            assert!(validate_dimensions(dimensions.0, dimensions.1).is_err());
        }
    }

    fn completed_terminal() -> Arc<Terminal> {
        let (commands, _) = mpsc::channel(1);
        Arc::new(Terminal {
            state: Mutex::new(State {
                session: TerminalSession {
                    id: "retained-terminal".into(),
                    profile_id: "server-a".into(),
                    title: "终端 1".into(),
                    status: "exited".into(),
                    exit_code: Some(0),
                    error: None,
                    cols: 80,
                    rows: 24,
                },
                output: Output::default(),
            }),
            sequence: 1,
            commands,
            cancel: CancellationToken::new(),
            task: Mutex::new(None),
        })
    }

    #[tokio::test]
    async fn exited_sessions_keep_output_and_enforce_profile_ownership_until_closed() {
        let manager = TerminalManager::new();
        let terminal = completed_terminal();
        terminal.output("保留的输出\r\n".as_bytes());
        manager
            .sessions
            .lock()
            .unwrap()
            .entries
            .insert(terminal.session().id, terminal);
        assert_eq!(manager.list("server-a").len(), 1);
        assert!(manager.list("server-b").is_empty());
        let read = manager.read("server-a", "retained-terminal", 0).unwrap();
        assert_eq!(
            STANDARD.decode(read.data).unwrap(),
            "保留的输出\r\n".as_bytes()
        );
        assert_eq!(read.session.status, "exited");
        assert!(manager.read("server-b", "retained-terminal", 0).is_err());
        assert!(manager
            .rename("server-b", "retained-terminal", "other")
            .is_err());
        assert!(manager
            .close("server-b", "retained-terminal")
            .await
            .is_err());
        assert_eq!(
            manager
                .rename("server-a", "retained-terminal", "  训练日志  ")
                .unwrap()
                .title,
            "训练日志"
        );
        for invalid in ["", "   ", "a\nb", &"a".repeat(81)] {
            assert!(manager
                .rename("server-a", "retained-terminal", invalid)
                .is_err());
        }
        assert!(manager
            .write("server-a", "retained-terminal", "!")
            .await
            .is_err());
        assert!(manager
            .write(
                "server-a",
                "retained-terminal",
                &STANDARD.encode(vec![0; INPUT_LIMIT + 1])
            )
            .await
            .is_err());
        assert!(manager
            .write(
                "server-a",
                "retained-terminal",
                &STANDARD.encode(b"echo x\r")
            )
            .await
            .is_err());
        manager
            .close("server-a", "retained-terminal")
            .await
            .unwrap();
        assert!(manager.list("server-a").is_empty());
        assert!(manager.read("server-a", "retained-terminal", 0).is_err());
    }

    #[tokio::test]
    async fn shutdown_clears_sessions_and_can_resume_after_a_failed_app_update() {
        let manager = TerminalManager::new();
        let terminal = completed_terminal();
        let cancel = terminal.cancel.clone();
        manager
            .sessions
            .lock()
            .unwrap()
            .entries
            .insert(terminal.session().id, terminal);
        manager.shutdown().await;
        assert!(cancel.is_cancelled());
        assert!(manager.list("server-a").is_empty());
        assert!(manager.cancelled.lock().unwrap().is_cancelled());
        manager.resume();
        assert!(!manager.cancelled.lock().unwrap().is_cancelled());
    }
}
