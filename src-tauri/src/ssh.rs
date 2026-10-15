//! Native SSH transport, command channels, and supervised TCP forwarding.
//!
//! Each connection owns an independently cancellable socket. Monitoring uses
//! allowlisted commands through `Executor`; the terminal tool opens separate
//! PTY shell channels on this same authenticated SSH transport.

use async_trait::async_trait;
use base64::Engine;
use russh::client::{self, KeyboardInteractiveAuthResponse};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{Channel, ChannelMsg, ChannelOpenFailure, Disconnect};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex as AsyncMutex, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};
use tokio_util::task::TaskTracker;

const CHANNEL_TIMEOUT: Duration = Duration::from_secs(8);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

pub type EventSink = Arc<dyn Fn(&str, Value) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<u32>,
}

#[async_trait]
pub trait Executor: Send + Sync {
    async fn exec(
        &self,
        command: &str,
        timeout: Duration,
        max_output: usize,
    ) -> Result<CommandOutput, String>;

    /// Run a command with a bounded local file as its standard input. This is
    /// intentionally transport-only; callers must validate their payload first.
    async fn exec_with_input(
        &self,
        _command: &str,
        _timeout: Duration,
        _max_output: usize,
        _input: tokio::fs::File,
        _input_size: u64,
    ) -> Result<CommandOutput, String> {
        Err("当前 SSH 执行器不支持文件输入".into())
    }
}

/// Unlike dropping a russh channel, dropping this guard explicitly sends CLOSE.
/// It also covers a caller dropping/aborting the command future during upload.
struct InputChannelGuard {
    writer: Arc<russh::ChannelWriteHalf<client::Msg>>,
    armed: bool,
}

impl InputChannelGuard {
    async fn close(&mut self) {
        let _ = timeout(Duration::from_secs(1), self.writer.close()).await;
        self.armed = false;
    }
}

impl Drop for InputChannelGuard {
    fn drop(&mut self) {
        if self.armed {
            let writer = self.writer.clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = timeout(Duration::from_secs(1), writer.close()).await;
                });
            }
        }
    }
}

#[derive(Debug)]
pub struct HostDecision {
    pub fingerprint: String,
    pub accepted: bool,
}

pub type TrustSender = mpsc::UnboundedSender<HostDecision>;
pub type TrustReceiver = mpsc::UnboundedReceiver<HostDecision>;

pub fn trust_channel() -> (TrustSender, TrustReceiver) {
    mpsc::unbounded_channel()
}

fn compatible_preferred() -> russh::Preferred {
    let mut preferred = russh::Preferred::default();
    // Preserve the old ssh2 default protocol set while retaining russh's
    // modern algorithm priority. Keep protocol extension markers at the end.
    let kex = preferred.kex.to_mut();
    let extension_at = kex
        .iter()
        .position(|name| {
            name.as_ref().starts_with("ext-info-") || name.as_ref().starts_with("kex-strict-")
        })
        .unwrap_or(kex.len());
    let extensions = kex.split_off(extension_at);
    for name in [
        russh::kex::ECDH_SHA2_NISTP256,
        russh::kex::ECDH_SHA2_NISTP384,
        russh::kex::ECDH_SHA2_NISTP521,
    ] {
        if !kex.contains(&name) {
            kex.push(name);
        }
    }
    kex.extend(extensions);
    for name in [russh::mac::HMAC_SHA1_ETM, russh::mac::HMAC_SHA1] {
        let mac = preferred.mac.to_mut();
        if !mac.contains(&name) {
            mac.push(name);
        }
    }
    let cipher = preferred.cipher.to_mut();
    if !cipher.contains(&russh::cipher::AES_128_GCM) {
        cipher.push(russh::cipher::AES_128_GCM);
    }
    preferred
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn text(value: &Value, name: &str, fallback: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_owned()
}

fn port(value: &Value, name: &str, fallback: u16) -> Result<u16, String> {
    let number = value
        .get(name)
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(Some(fallback as u64));
    match number {
        Some(p @ 1..=65535) => Ok(p as u16),
        _ => Err(format!("{name} 端口必须在 1 到 65535 之间")),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TunnelConfig {
    id: String,
    name: String,
    direction: String,
    bind_host: String,
    bind_port: u16,
    target_host: String,
    target_port: u16,
    enabled: bool,
    health_interval_sec: u64,
}

impl TunnelConfig {
    fn parse(value: Value) -> Result<Self, String> {
        let id = text(&value, "id", "");
        if id.is_empty() {
            return Err("隧道缺少 ID".into());
        }
        let direction = text(&value, "direction", "");
        if direction != "local" && direction != "remote" {
            return Err("无效的隧道方向".into());
        }
        let interval = value
            .get("healthIntervalSec")
            .and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(10.0);
        Ok(Self {
            id,
            name: text(&value, "name", "隧道"),
            direction,
            bind_host: text(&value, "bindHost", "127.0.0.1"),
            bind_port: port(&value, "bindPort", 0)?,
            target_host: text(&value, "targetHost", "127.0.0.1"),
            target_port: port(&value, "targetPort", 0)?,
            enabled: value.get("enabled") != Some(&Value::Bool(false)),
            health_interval_sec: interval.clamp(2.0, 300.0) as u64,
        })
    }
}

struct Probe {
    challenge: Vec<u8>,
    ack: Vec<u8>,
    active: AtomicBool,
    target_error: Mutex<Option<String>>,
}

struct Tunnel {
    config: RwLock<TunnelConfig>,
    state: Mutex<Value>,
    generation: AtomicU64,
    active: AtomicBool,
    remote_bound: AtomicBool,
    cancel: Mutex<CancellationToken>,
    forward_cancel: Mutex<CancellationToken>,
    job: AsyncMutex<Option<JoinHandle<()>>>,
    probe: Mutex<Option<Arc<Probe>>>,
    control: AsyncMutex<()>,
}

impl Tunnel {
    fn new(config: TunnelConfig) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(json!({"id": config.id, "status":"stopped", "error":null,
                "lastChecked":null, "healthVerified":false, "healthStatus":"pending"})),
            config: RwLock::new(config),
            generation: AtomicU64::new(0),
            active: AtomicBool::new(false),
            remote_bound: AtomicBool::new(false),
            cancel: Mutex::new(CancellationToken::new()),
            forward_cancel: Mutex::new(CancellationToken::new()),
            job: AsyncMutex::new(None),
            probe: Mutex::new(None),
            control: AsyncMutex::new(()),
        })
    }

    fn clear_probe(&self) {
        if let Some(probe) = self.probe.lock().unwrap().take() {
            probe.active.store(false, Ordering::Release);
        }
    }
}

struct Shared {
    profile_id: String,
    host: String,
    port: u16,
    events: EventSink,
    tunnels: Mutex<HashMap<String, Arc<Tunnel>>>,
    closed: CancellationToken,
    transport_cancel: CancellationToken,
    failure: Mutex<String>,
    handshake_deadline: AtomicU64,
    handshake_done: AtomicBool,
}

impl Shared {
    fn emit(&self, mut value: Value) {
        value["profileId"] = json!(self.profile_id);
        (self.events)("transport", value);
    }

    fn event(&self, level: &str, message: impl AsRef<str>) {
        self.emit(json!({"type":"event", "level":level, "message":message.as_ref()}));
    }

    fn state(&self, tunnel: &Tunnel, patch: Value) {
        let mut state = tunnel.state.lock().unwrap();
        if let Some(fields) = patch.as_object() {
            for (key, value) in fields {
                state[key] = value.clone();
            }
        }
        let snapshot = state.clone();
        drop(state);
        self.emit(json!({"type":"tunnel", "state":snapshot}));
    }

    fn fail(&self, reason: impl Into<String>) {
        let mut failure = self.failure.lock().unwrap();
        if failure.is_empty() {
            *failure = reason.into();
        }
        drop(failure);
        self.closed.cancel();
        self.transport_cancel.cancel();
        for tunnel in self.tunnels.lock().unwrap().values() {
            tunnel.cancel.lock().unwrap().cancel();
        }
    }

    fn reason(&self, fallback: impl ToString) -> String {
        let reason = self.failure.lock().unwrap();
        if reason.is_empty() {
            fallback.to_string()
        } else {
            reason.clone()
        }
    }
}

/// Cancelling a connection interrupts the physical socket even when a server
/// never replies to a forwarding/authentication request.
struct CancelIo {
    socket: TcpStream,
    cancelled: Pin<Box<WaitForCancellationFutureOwned>>,
}

impl CancelIo {
    fn new(socket: TcpStream, cancel: CancellationToken) -> Self {
        Self {
            socket,
            cancelled: Box::pin(cancel.cancelled_owned()),
        }
    }

    fn poll_cancel(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        if self.cancelled.as_mut().poll(cx).is_ready() {
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "SSH connection cancelled",
            ))
        } else {
            Ok(())
        }
    }
}

impl AsyncRead for CancelIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if let Err(error) = self.poll_cancel(cx) {
            return Poll::Ready(Err(error));
        }
        Pin::new(&mut self.socket).poll_read(cx, buf)
    }
}

impl AsyncWrite for CancelIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if let Err(error) = self.poll_cancel(cx) {
            return Poll::Ready(Err(error));
        }
        Pin::new(&mut self.socket).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if let Err(error) = self.poll_cancel(cx) {
            return Poll::Ready(Err(error));
        }
        Pin::new(&mut self.socket).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.socket).poll_shutdown(cx)
    }
}

struct ConnectGuard(Option<CancellationToken>);
impl Drop for ConnectGuard {
    fn drop(&mut self) {
        if let Some(token) = self.0.take() {
            token.cancel();
        }
    }
}

struct Handler {
    shared: Arc<Shared>,
    trusted: HashSet<String>,
    trust: TrustReceiver,
}

impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // The digest matches ssh2's SHA256 of the SSH wire-format key. A host
        // certificate must be fingerprinted as a certificate, not its leaf key.
        let public = server_key.public_key();
        let fingerprint = if let Some(certificate) = server_key.certificate() {
            let bytes = certificate
                .to_bytes()
                .map_err(|_| russh::Error::UnknownKey)?;
            format!(
                "SHA256:{}",
                base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(bytes))
            )
        } else {
            public.fingerprint(HashAlg::Sha256).to_string()
        };
        if self.trusted.contains(&fingerprint) {
            return Ok(true);
        }
        self.shared
            .handshake_deadline
            .store(now() + 120_000, Ordering::Release);
        self.shared
            .emit(json!({"type":"status", "status":"awaiting-trust"}));
        self.shared
            .emit(json!({"type":"host-key", "fingerprint":fingerprint,
            "keyType": public.algorithm().to_string(), "host":self.shared.host,
            "port":self.shared.port, "changed":!self.trusted.is_empty()}));
        let decision = timeout(Duration::from_secs(120), async {
            loop {
                tokio::select! {
                    _ = self.shared.transport_cancel.cancelled() => return None,
                    decision = self.trust.recv() => match decision {
                        Some(decision) if decision.fingerprint == fingerprint => return Some(decision.accepted),
                        Some(_) => continue,
                        None => return None,
                    }
                }
            }
        }).await;
        self.shared
            .handshake_deadline
            .store(now() + 20_000, Ordering::Release);
        match decision {
            Ok(Some(true)) => {
                self.trusted.insert(fingerprint);
                self.shared
                    .emit(json!({"type":"status", "status":"connecting"}));
                Ok(true)
            }
            Ok(Some(false)) => {
                *self.shared.failure.lock().unwrap() = "已拒绝服务器指纹".into();
                Ok(false)
            }
            _ => {
                *self.shared.failure.lock().unwrap() =
                    "服务器指纹确认超时或连接已取消，请重新连接".into();
                Ok(false)
            }
        }
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<client::Msg>,
        connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let tunnel = self
            .shared
            .tunnels
            .lock()
            .unwrap()
            .values()
            .find(|tunnel| {
                let config = tunnel.config.read().unwrap();
                config.direction == "remote"
                    && config.enabled
                    && config.bind_port as u32 == connected_port
                    && tunnel.active.load(Ordering::Acquire)
                    && (matches!(
                        config.bind_host.as_str(),
                        "" | "0.0.0.0" | "::" | "localhost"
                    ) || config.bind_host == connected_address)
            })
            .cloned();
        let Some(tunnel) = tunnel else {
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        let shared = self.shared.clone();
        let token = tunnel.forward_cancel.lock().unwrap().clone();
        // Never wait for a target connect inside the SSH protocol callback:
        // doing so would block all unrelated channels on this connection.
        tokio::spawn(async move {
            let config = tunnel.config.read().unwrap().clone();
            let connected = tokio::select! {
                _ = token.cancelled() => return,
                result = timeout(CHANNEL_TIMEOUT, TcpStream::connect((config.target_host.as_str(), config.target_port))) => result,
            };
            let socket = match connected {
                Ok(Ok(socket)) => socket,
                result => {
                    let error = match result {
                        Ok(Err(error)) => error.to_string(),
                        _ => "本机目标连接超时".into(),
                    };
                    if let Some(probe) = tunnel.probe.lock().unwrap().as_ref() {
                        *probe.target_error.lock().unwrap() = Some(error.clone());
                    }
                    target_failed(&shared, &tunnel, &error);
                    reply.reject(ChannelOpenFailure::ConnectFailed).await;
                    return;
                }
            };
            if token.is_cancelled() {
                return;
            }
            reply.accept().await;
            let probe = tunnel.probe.lock().unwrap().clone();
            let _ = bridge_remote(socket, channel, probe, token).await;
        });
        Ok(())
    }

    async fn disconnected(
        &mut self,
        reason: client::DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        match reason {
            client::DisconnectReason::ReceivedDisconnect(info) => {
                self.shared.fail(if info.message.is_empty() {
                    "服务器关闭了 SSH 连接".into()
                } else {
                    info.message
                });
                Ok(())
            }
            client::DisconnectReason::Error(error) => {
                self.shared.fail(error.to_string());
                Err(error)
            }
        }
    }
}

pub struct Connection {
    handle: Arc<client::Handle<Handler>>,
    shared: Arc<Shared>,
    lifecycle: AsyncMutex<()>,
    terminal_opens: Arc<Semaphore>,
}

impl Connection {
    pub async fn connect(
        profile: Value,
        secrets: Value,
        trusted: Vec<String>,
        events: EventSink,
        trust: TrustReceiver,
    ) -> Result<Arc<Self>, String> {
        let profile_id = text(&profile, "id", "");
        let host = text(&profile, "host", "");
        let username = text(&profile, "username", "");
        if profile_id.is_empty() || host.is_empty() || username.is_empty() {
            return Err("连接配置不完整".into());
        }
        let ssh_port = port(&profile, "port", 22)?;
        let auth_type = text(&profile, "authType", "");
        if auth_type != "password" && auth_type != "key" {
            return Err("认证方式无效".into());
        }
        let password = text(&secrets, "password", "");
        let private_key = if auth_type == "key" {
            let path = text(&profile, "keyPath", "").chars().filter(|c| !matches!(*c, '\u{200e}'|'\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')).collect::<String>();
            let path = path.trim();
            if path.is_empty() {
                return Err("请选择 SSH 私钥文件".into());
            }
            let metadata = tokio::fs::metadata(path)
                .await
                .map_err(|e| format!("读取私钥失败：{e}"))?;
            if metadata.len() > 1_048_576 {
                return Err("私钥文件过大".into());
            }
            let key = tokio::fs::read(path)
                .await
                .map_err(|e| format!("读取私钥失败：{e}"))?;
            if key.len() > 1_048_576 {
                return Err("私钥文件过大".into());
            }
            let key = String::from_utf8(key).map_err(|_| "SSH 私钥文件格式不正确")?;
            let passphrase = text(&secrets, "passphrase", "");
            Some(
                tokio::task::spawn_blocking(move || {
                    russh::keys::decode_secret_key(
                        &key,
                        (!passphrase.is_empty()).then_some(passphrase.as_str()),
                    )
                    .map_err(|e| format!("解析 SSH 私钥失败，请检查文件和口令：{e}"))
                })
                .await
                .map_err(|e| e.to_string())??,
            )
        } else {
            if password.is_empty() {
                return Err("请输入 SSH 密码".into());
            }
            None
        };
        let rules = profile
            .get("tunnels")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        // Validate the entire set before opening the network connection.
        let configs = parse_rules(rules)?;
        let shared = Arc::new(Shared {
            profile_id,
            host: host.clone(),
            port: ssh_port,
            events,
            tunnels: Mutex::new(HashMap::new()),
            closed: CancellationToken::new(),
            transport_cancel: CancellationToken::new(),
            failure: Mutex::new(String::new()),
            handshake_deadline: AtomicU64::new(now() + 20_000),
            handshake_done: AtomicBool::new(false),
        });
        let mut guard = ConnectGuard(Some(shared.transport_cancel.clone()));
        let deadline_shared = Arc::downgrade(&shared);
        tokio::spawn(async move {
            loop {
                sleep(Duration::from_millis(200)).await;
                let Some(shared) = deadline_shared.upgrade() else {
                    break;
                };
                if shared.handshake_done.load(Ordering::Acquire)
                    || shared.transport_cancel.is_cancelled()
                {
                    break;
                }
                if now() >= shared.handshake_deadline.load(Ordering::Acquire) {
                    shared.fail("SSH 连接或认证超时");
                    break;
                }
            }
        });
        let socket = timeout(
            CONNECT_TIMEOUT,
            TcpStream::connect((host.as_str(), ssh_port)),
        )
        .await
        .map_err(|_| "SSH 连接超时".to_string())?
        .map_err(|e| e.to_string())?;
        let _ = socket.set_nodelay(true);
        let handler = Handler {
            shared: shared.clone(),
            trusted: trusted.into_iter().collect(),
            trust,
        };
        let config = client::Config {
            preferred: compatible_preferred(),
            keepalive_interval: Some(Duration::from_secs(5)),
            keepalive_max: 3,
            nodelay: true,
            ..Default::default()
        };
        let mut handle = client::connect_stream(
            Arc::new(config),
            CancelIo::new(socket, shared.transport_cancel.clone()),
            handler,
        )
        .await
        .map_err(|e| shared.reason(e))?;
        shared
            .handshake_deadline
            .store(now() + 20_000, Ordering::Release);
        let auth = async {
            if let Some(key) = private_key {
                let hash = handle
                    .best_supported_rsa_hash()
                    .await
                    .map_err(|e| e.to_string())?
                    .flatten();
                let result = handle
                    .authenticate_publickey(
                        &username,
                        PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                return Ok::<bool, String>(result.success());
            }
            let result = handle
                .authenticate_password(&username, &password)
                .await
                .map_err(|e| e.to_string())?;
            if result.success() {
                return Ok(true);
            }
            let mut response = handle
                .authenticate_keyboard_interactive_start(&username, None)
                .await
                .map_err(|e| e.to_string())?;
            for _ in 0..8 {
                match response {
                    KeyboardInteractiveAuthResponse::Success => return Ok(true),
                    KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
                    KeyboardInteractiveAuthResponse::InfoRequest { ref prompts, .. } => {
                        if prompts.len() > 1 {
                            return Err("服务器要求多个交互认证项，当前连接仅支持单密码认证".into());
                        }
                        let answers = if prompts.is_empty() {
                            vec![]
                        } else {
                            vec![password.clone()]
                        };
                        response = handle
                            .authenticate_keyboard_interactive_respond(answers)
                            .await
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
            Err("SSH 交互认证次数超过限制".into())
        };
        let authenticated = timeout(CONNECT_TIMEOUT, auth)
            .await
            .map_err(|_| "SSH 认证超时".to_string())??;
        if !authenticated {
            return Err("SSH 认证失败，请检查用户名和登录凭据".into());
        }
        shared.handshake_done.store(true, Ordering::Release);
        let connection = Arc::new(Self {
            handle: Arc::new(handle),
            shared,
            lifecycle: AsyncMutex::new(()),
            terminal_opens: Arc::new(Semaphore::new(16)),
        });
        guard.0 = None;
        for config in configs {
            let tunnel = Tunnel::new(config.clone());
            connection
                .shared
                .tunnels
                .lock()
                .unwrap()
                .insert(config.id, tunnel.clone());
            if config.enabled {
                connection.start_tunnel(tunnel).await;
            }
        }
        connection
            .shared
            .emit(json!({"type":"tunnels","states":connection.tunnel_states()}));
        let weak = Arc::downgrade(&connection);
        tokio::spawn(async move {
            loop {
                sleep(Duration::from_millis(200)).await;
                let Some(connection) = weak.upgrade() else {
                    break;
                };
                if connection.shared.closed.is_cancelled() {
                    break;
                }
                if connection.handle.is_closed() {
                    connection.shared.fail("SSH 连接中断");
                    break;
                }
            }
        });
        Ok(connection)
    }

    pub fn is_closed(&self) -> bool {
        self.shared.closed.is_cancelled() || self.handle.is_closed()
    }

    pub async fn closed(&self) -> String {
        self.shared.closed.cancelled().await;
        self.shared.reason("SSH 连接中断")
    }

    /// Keep late confirmations alive long enough to close them explicitly.
    /// Both per-transport and global permits bound these pending opens even
    /// if a server never acknowledges a channel request.
    pub async fn open_terminal_channel(
        &self,
        cancelled: &CancellationToken,
        global_permit: OwnedSemaphorePermit,
    ) -> Result<Channel<client::Msg>, String> {
        if self.is_closed() {
            return Err("SSH 连接已关闭".into());
        }
        let permit = self
            .terminal_opens
            .clone()
            .try_acquire_owned()
            .map_err(|_| "终端通道仍在建立，请稍后重试")?;
        let handle = self.handle.clone();
        let closed = self.shared.closed.clone();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _permits = (permit, global_permit);
            let result = tokio::select! {
                _ = closed.cancelled() => return,
                result = handle.channel_open_session() => result,
            };
            if let Err(Ok(channel)) = sender.send(result) {
                let _ = timeout(Duration::from_secs(1), channel.close()).await;
            }
        });
        tokio::select! {
            _ = cancelled.cancelled() => Err("终端已关闭".into()),
            _ = self.shared.closed.cancelled() => Err("SSH 连接已关闭".into()),
            result = timeout(CHANNEL_TIMEOUT, receiver) => result
                .map_err(|_| "终端通道建立超时")?
                .map_err(|_| "SSH 连接已关闭")?
                .map_err(|error| format!("终端通道建立失败：{error}")),
        }
    }

    pub fn tunnel_states(&self) -> Vec<Value> {
        let mut states = self
            .shared
            .tunnels
            .lock()
            .unwrap()
            .values()
            .map(|tunnel| tunnel.state.lock().unwrap().clone())
            .collect::<Vec<_>>();
        states.sort_by_key(|state| text(state, "id", ""));
        states
    }

    pub async fn close(&self) {
        let _lifecycle = self.lifecycle.lock().await;
        let tunnels = self
            .shared
            .tunnels
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        // Cancel every task immediately; then await individual remote unbinds.
        for tunnel in &tunnels {
            tunnel.cancel.lock().unwrap().cancel();
        }
        let mut jobs = tokio::task::JoinSet::new();
        for tunnel in tunnels {
            let shared = self.shared.clone();
            jobs.spawn(async move { stop_tunnel(&shared, &tunnel).await });
        }
        while jobs.join_next().await.is_some() {}
        let _ = timeout(
            Duration::from_secs(2),
            self.handle
                .disconnect(Disconnect::ByApplication, "Lion SSH disconnected", "zh-CN"),
        )
        .await;
        self.shared.fail("连接已关闭");
    }

    async fn start_tunnel(self: &Arc<Self>, tunnel: Arc<Tunnel>) {
        let token = self.shared.closed.child_token();
        *tunnel.cancel.lock().unwrap() = token.clone();
        let handle = self.handle.clone();
        let shared = self.shared.clone();
        let runtime = tunnel.clone();
        let task = tokio::spawn(async move {
            run_tunnel(handle, shared, runtime, token).await;
        });
        *tunnel.job.lock().await = Some(task);
    }

    pub async fn update_tunnels(self: &Arc<Self>, values: Vec<Value>) -> Result<(), String> {
        let configs = parse_rules(values)?;
        let _lifecycle = self.lifecycle.lock().await;
        let desired: HashMap<_, _> = configs
            .into_iter()
            .map(|config| (config.id.clone(), config))
            .collect();
        let old = self
            .shared
            .tunnels
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for tunnel in old {
            let config = tunnel.config.read().unwrap().clone();
            if desired.get(&config.id) != Some(&config) {
                stop_tunnel(&self.shared, &tunnel).await;
                self.shared.tunnels.lock().unwrap().remove(&config.id);
            }
        }
        for (id, config) in desired {
            if self.shared.tunnels.lock().unwrap().contains_key(&id) {
                continue;
            }
            let tunnel = Tunnel::new(config.clone());
            self.shared
                .tunnels
                .lock()
                .unwrap()
                .insert(id, tunnel.clone());
            if config.enabled && !self.is_closed() {
                self.start_tunnel(tunnel).await;
            }
        }
        self.shared
            .emit(json!({"type":"tunnels","states":self.tunnel_states()}));
        Ok(())
    }

    pub async fn set_tunnel(self: &Arc<Self>, id: &str, enabled: bool) -> Result<(), String> {
        let _lifecycle = self.lifecycle.lock().await;
        let tunnel = self
            .shared
            .tunnels
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("隧道不存在")?;
        let _control = tunnel.control.lock().await;
        tunnel.config.write().unwrap().enabled = enabled;
        stop_tunnel(&self.shared, &tunnel).await;
        if enabled && !self.is_closed() {
            self.start_tunnel(tunnel.clone()).await;
        }
        Ok(())
    }

    pub async fn restart_tunnel(self: &Arc<Self>, id: &str) -> Result<(), String> {
        let _lifecycle = self.lifecycle.lock().await;
        let tunnel = self
            .shared
            .tunnels
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("隧道不存在")?;
        let _control = tunnel.control.lock().await;
        stop_tunnel(&self.shared, &tunnel).await;
        if tunnel.config.read().unwrap().enabled && !self.is_closed() {
            self.start_tunnel(tunnel.clone()).await;
        }
        Ok(())
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.shared.fail("连接已关闭");
    }
}

fn parse_rules(values: Vec<Value>) -> Result<Vec<TunnelConfig>, String> {
    let configs = values
        .into_iter()
        .map(TunnelConfig::parse)
        .collect::<Result<Vec<_>, _>>()?;
    let mut ids = HashSet::new();
    if configs.iter().any(|config| !ids.insert(config.id.clone())) {
        return Err("隧道 ID 重复".into());
    }
    Ok(configs)
}

#[async_trait]
impl Executor for Connection {
    async fn exec(
        &self,
        command: &str,
        duration: Duration,
        max_output: usize,
    ) -> Result<CommandOutput, String> {
        if self.is_closed() {
            return Err("SSH 未连接".into());
        }
        let deadline = tokio::time::Instant::now() + duration;
        let handle = self.handle.clone();
        let (opened, received) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = handle.channel_open_session().await;
            // A cancelled query can receive a late open confirmation. Close
            // that channel explicitly instead of leaving it on the server.
            if let Err(Ok(channel)) = opened.send(result) {
                let _ = channel.close().await;
            }
        });
        let mut channel = tokio::select! {
            _ = self.shared.closed.cancelled() => return Err("SSH 连接已关闭".into()),
            result = tokio::time::timeout_at(deadline, received) => result.map_err(|_| "SSH 通道建立超时")?.map_err(|_| "SSH 连接已关闭")?.map_err(|e| e.to_string())?,
        };
        let operation = async {
            channel
                .exec(true, command)
                .await
                .map_err(|e| e.to_string())?;
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let mut exit_code = None;
            while let Some(message) = channel.wait().await {
                match message {
                    ChannelMsg::Data { data } => {
                        if stdout.len().saturating_add(data.len()) > max_output {
                            return Err("远程指令输出超过限制".into());
                        }
                        stdout.extend_from_slice(&data);
                    }
                    ChannelMsg::ExtendedData { data, .. } => {
                        // Keep the original bounded stderr tail; decode only
                        // after gathering bytes to preserve split UTF-8 text.
                        stderr.extend_from_slice(&data);
                        if stderr.len() > 8192 {
                            stderr.drain(..stderr.len() - 8192);
                        }
                    }
                    ChannelMsg::ExitStatus { exit_status } => exit_code = Some(exit_status),
                    ChannelMsg::ExitSignal { .. } => {
                        if exit_code.is_none() {
                            exit_code = Some(255);
                        }
                    }
                    ChannelMsg::Close => break,
                    _ => {}
                }
            }
            if self.is_closed() {
                return Err("SSH 连接已关闭".into());
            }
            Ok(CommandOutput {
                stdout: String::from_utf8_lossy(&stdout).into_owned(),
                stderr: String::from_utf8_lossy(&stderr)
                    .chars()
                    .rev()
                    .take(2000)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect(),
                exit_code,
            })
        };
        let result = tokio::select! {
            _ = self.shared.closed.cancelled() => Err("SSH 连接已关闭".into()),
            result = tokio::time::timeout_at(deadline, operation) => result.unwrap_or_else(|_| Err("远程指令执行超时".into())),
        };
        // Explicitly close even on timeout/cancellation; dropping a bare russh
        // Channel does not otherwise guarantee that the remote command ends.
        let _ = timeout(Duration::from_secs(1), channel.close()).await;
        result
    }

    async fn exec_with_input(
        &self,
        command: &str,
        duration: Duration,
        max_output: usize,
        mut input: tokio::fs::File,
        input_size: u64,
    ) -> Result<CommandOutput, String> {
        if self.is_closed() {
            return Err("SSH 未连接".into());
        }
        let deadline = tokio::time::Instant::now() + duration;
        let metadata = tokio::select! {
            _ = self.shared.closed.cancelled() => return Err("SSH 连接已关闭".into()),
            result = tokio::time::timeout_at(deadline, input.metadata()) => {
                result.map_err(|_| "读取上传文件超时")?.map_err(|e| format!("读取上传文件失败：{e}"))?
            }
        };
        if !metadata.is_file() || metadata.len() != input_size {
            return Err("上传文件长度与预期不符".into());
        }
        let handle = self.handle.clone();
        let (opened, received) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = handle.channel_open_session().await;
            // Cover a confirmation arriving after the calling future timed out
            // or was dropped, just as with commands without standard input.
            if let Err(Ok(channel)) = opened.send(result) {
                let _ = channel.close().await;
            }
        });
        let channel = tokio::select! {
            _ = self.shared.closed.cancelled() => return Err("SSH 连接已关闭".into()),
            result = tokio::time::timeout_at(deadline, received) => result.map_err(|_| "SSH 通道建立超时")?.map_err(|_| "SSH 连接已关闭")?.map_err(|e| e.to_string())?,
        };
        let (mut reader, writer) = channel.split();
        let writer = Arc::new(writer);
        let mut guard = InputChannelGuard {
            writer: writer.clone(),
            armed: true,
        };
        let input_complete = AtomicBool::new(false);
        let operation = async {
            writer
                .exec(true, command)
                .await
                .map_err(|e| e.to_string())?;
            let upload = async {
                let mut remaining = input_size;
                let mut buffer = vec![0u8; 64 * 1024];
                while remaining > 0 {
                    let limit = remaining.min(buffer.len() as u64) as usize;
                    let count = input
                        .read(&mut buffer[..limit])
                        .await
                        .map_err(|e| format!("读取上传文件失败：{e}"))?;
                    if count == 0 {
                        return Err("上传文件在传输过程中缩短".into());
                    }
                    writer
                        .data(&buffer[..count])
                        .await
                        .map_err(|e| format!("上传文件失败：{e}"))?;
                    remaining -= count as u64;
                }
                // Re-check EOF even after validating metadata: a file may have
                // changed, or a caller may have supplied a non-zero cursor.
                if input
                    .read(&mut buffer[..1])
                    .await
                    .map_err(|e| format!("读取上传文件失败：{e}"))?
                    != 0
                {
                    return Err("上传文件在传输过程中变长".into());
                }
                writer.eof().await.map_err(|e| e.to_string())?;
                input_complete.store(true, Ordering::Release);
                Ok::<(), String>(())
            };
            let output = async {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                let mut exit_code = None;
                while let Some(message) = reader.wait().await {
                    match message {
                        ChannelMsg::Data { data } => {
                            if stdout.len().saturating_add(data.len()) > max_output {
                                return Err("远程指令输出超过限制".into());
                            }
                            stdout.extend_from_slice(&data);
                        }
                        ChannelMsg::ExtendedData { data, .. } => {
                            stderr.extend_from_slice(&data);
                            if stderr.len() > 8192 {
                                stderr.drain(..stderr.len() - 8192);
                            }
                        }
                        ChannelMsg::ExitStatus { exit_status } => {
                            if !input_complete.load(Ordering::Acquire) {
                                let detail = String::from_utf8_lossy(&stderr)
                                    .chars()
                                    .rev()
                                    .take(2000)
                                    .collect::<String>()
                                    .chars()
                                    .rev()
                                    .collect::<String>();
                                return Err(format!("文件传输完成前远程指令已退出（退出码 {exit_status}）：{detail}"));
                            }
                            exit_code = Some(exit_status);
                        }
                        ChannelMsg::ExitSignal { .. } => {
                            if !input_complete.load(Ordering::Acquire) {
                                return Err("文件传输完成前远程指令被信号终止".into());
                            }
                            if exit_code.is_none() {
                                exit_code = Some(255);
                            }
                        }
                        ChannelMsg::Failure => return Err("服务器拒绝远程指令".into()),
                        ChannelMsg::Eof if !input_complete.load(Ordering::Acquire) => {
                            return Err("文件传输完成前远程通道已结束输出".into());
                        }
                        ChannelMsg::Close => break,
                        _ => {}
                    }
                }
                if !input_complete.load(Ordering::Acquire) {
                    return Err("文件传输完成前远程通道已关闭".into());
                }
                if self.is_closed() {
                    return Err("SSH 连接已关闭".into());
                }
                Ok(CommandOutput {
                    stdout: String::from_utf8_lossy(&stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&stderr)
                        .chars()
                        .rev()
                        .take(2000)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect(),
                    exit_code,
                })
            };
            // Drain output while sending input. Either peer can otherwise fill
            // its SSH channel window and deadlock on a sufficiently large file.
            tokio::try_join!(upload, output).map(|(_, output)| output)
        };
        let result = tokio::select! {
            _ = self.shared.closed.cancelled() => Err("SSH 连接已关闭".into()),
            result = tokio::time::timeout_at(deadline, operation) => result.unwrap_or_else(|_| Err("远程指令执行超时".into())),
        };
        guard.close().await;
        result
    }
}

async fn stop_tunnel(shared: &Arc<Shared>, tunnel: &Arc<Tunnel>) {
    tunnel.cancel.lock().unwrap().cancel();
    tunnel.active.store(false, Ordering::Release);
    tunnel.clear_probe();
    if let Some(mut task) = tunnel.job.lock().await.take() {
        if timeout(Duration::from_secs(10), &mut task).await.is_err() {
            shared.fail("旧远端监听未确认释放，将关闭 SSH 会话后恢复");
            task.abort();
            let _ = task.await;
        }
    }
    shared.state(
        tunnel,
        json!({"status":"stopped", "error":null, "healthVerified":false,"healthStatus":"pending"}),
    );
}

fn target_failed(shared: &Shared, tunnel: &Tunnel, error: &str) {
    if !tunnel.active.load(Ordering::Acquire) {
        return;
    }
    shared.state(
        tunnel,
        json!({"status":"degraded", "error":format!("目标不可用：{error}"),
        "healthVerified":false,"healthStatus":"target-unavailable","lastChecked":now()}),
    );
}

async fn open_forward(
    handle: &Arc<client::Handle<Handler>>,
    host: &str,
    port: u16,
    origin: &str,
    origin_port: u16,
) -> Result<Channel<client::Msg>, ForwardError> {
    let handle = handle.clone();
    let host = host.to_owned();
    let origin = origin.to_owned();
    let (opened, received) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let result = handle
            .channel_open_direct_tcpip(host, port as u32, origin, origin_port as u32)
            .await;
        if let Err(Ok(channel)) = opened.send(result) {
            let _ = channel.close().await;
        }
    });
    match timeout(CHANNEL_TIMEOUT, received).await {
        Ok(Ok(Ok(channel))) => Ok(channel),
        Ok(Ok(Err(error))) => {
            let restricted = matches!(
                error,
                russh::Error::ChannelOpenFailure(ChannelOpenFailure::AdministrativelyProhibited)
            );
            let refused = matches!(
                error,
                russh::Error::ChannelOpenFailure(ChannelOpenFailure::ConnectFailed)
            );
            Err(ForwardError {
                message: error.to_string(),
                restricted,
                fault: refused,
            })
        }
        Ok(Err(_)) => Err(ForwardError {
            message: "SSH 连接已关闭".into(),
            restricted: false,
            fault: false,
        }),
        Err(_) => Err(ForwardError {
            message: "转发通道连接超时".into(),
            restricted: false,
            fault: false,
        }),
    }
}

struct ForwardError {
    message: String,
    restricted: bool,
    fault: bool,
}

async fn run_tunnel(
    handle: Arc<client::Handle<Handler>>,
    shared: Arc<Shared>,
    tunnel: Arc<Tunnel>,
    token: CancellationToken,
) {
    let config = tunnel.config.read().unwrap().clone();
    let mut retry_attempt = 0u32;
    loop {
        if token.is_cancelled() || shared.closed.is_cancelled() {
            break;
        }
        let generation = tunnel.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let runtime_token = token.child_token();
        *tunnel.forward_cancel.lock().unwrap() = runtime_token.clone();
        let tracker = TaskTracker::new();
        shared.state(&tunnel, json!({"status":"starting", "error":null,"healthVerified":false,"healthStatus":"pending"}));
        let startup = if config.direction == "local" {
            match timeout(
                CHANNEL_TIMEOUT,
                TcpListener::bind((config.bind_host.as_str(), config.bind_port)),
            )
            .await
            {
                Ok(Ok(listener)) => {
                    let handle = handle.clone();
                    let shared = shared.clone();
                    let tunnel = tunnel.clone();
                    let cancel = runtime_token.clone();
                    let connections = tracker.clone();
                    tracker.spawn(async move {
                        loop {
                            let accepted = tokio::select! {
                                _ = cancel.cancelled() => break,
                                result = listener.accept() => result,
                            };
                            match accepted {
                                Ok((socket, origin)) => {
                                    let handle = handle.clone(); let shared = shared.clone(); let tunnel = tunnel.clone();
                                    let token = cancel.clone();
                                    connections.spawn(async move {
                                        let rule = tunnel.config.read().unwrap().clone();
                                        let origin_address = origin.ip().to_string();
                                        let opened = tokio::select! {
                                            _ = token.cancelled() => return,
                                            result = open_forward(&handle, &rule.target_host, rule.target_port, &origin_address, origin.port()) => result,
                                        };
                                        match opened {
                                            Ok(channel) => { let _ = bridge(socket, channel, token).await; }
                                            Err(error) => target_failed(&shared, &tunnel, &error.message),
                                        }
                                    });
                                }
                                Err(error) => {
                                    shared.state(&tunnel, json!({"status":"recovering","error":error.to_string(),"healthVerified":false}));
                                    cancel.cancel();
                                    break;
                                }
                            }
                        }
                    });
                    Ok(())
                }
                Ok(Err(error)) => Err(error.to_string()),
                Err(_) => Err("本机端口监听超时".into()),
            }
        } else {
            // The handler must recognize a forwarded channel immediately if
            // it arrives in the same packet as the successful bind response.
            tunnel.active.store(true, Ordering::Release);
            match timeout(
                CHANNEL_TIMEOUT,
                handle.tcpip_forward(&config.bind_host, config.bind_port as u32),
            )
            .await
            {
                Ok(Ok(_)) => {
                    tunnel.remote_bound.store(true, Ordering::Release);
                    Ok(())
                }
                Ok(Err(error)) => Err(error.to_string()),
                Err(_) => {
                    shared.fail("远端监听请求超时，将关闭 SSH 会话后恢复");
                    Err("远端监听请求超时".into())
                }
            }
        };
        let mut needs_recovery = false;
        match startup {
            Ok(()) if !token.is_cancelled() && !shared.closed.is_cancelled() => {
                tunnel.active.store(true, Ordering::Release);
                retry_attempt = 0;
                shared.state(&tunnel, json!({"status":"running", "error":if config.direction=="remote" {Some("监听已建立，等待连通性检测")}else{None}}));
                shared.event("success", format!("{}：隧道已建立", config.name));
                loop {
                    if token.is_cancelled()
                        || shared.closed.is_cancelled()
                        || runtime_token.is_cancelled()
                    {
                        break;
                    }
                    let health = tokio::select! {
                        _ = token.cancelled() => break,
                        _ = shared.closed.cancelled() => break,
                        result = check_health(&handle, &tunnel, &config) => result,
                    };
                    if generation != tunnel.generation.load(Ordering::Acquire) {
                        break;
                    }
                    shared.state(&tunnel, json!({"status":health.status,"error":health.error,"healthVerified":health.verified,
                        "healthStatus":health.health_status,"lastChecked":now()}));
                    if health.recover {
                        needs_recovery = true;
                        break;
                    }
                    tokio::select! {
                        _ = token.cancelled() => break,
                        _ = shared.closed.cancelled() => break,
                        _ = runtime_token.cancelled() => {needs_recovery = true; break;},
                        _ = sleep(Duration::from_secs(config.health_interval_sec)) => {}
                    }
                }
            }
            Ok(()) => {}
            Err(error) => {
                needs_recovery = true;
                shared.state(&tunnel, json!({"status":"recovering","error":error,"healthVerified":false,"healthStatus":"pending"}));
                shared.event("warning", format!("{}：{error}", config.name));
            }
        }
        tunnel.active.store(false, Ordering::Release);
        tunnel.clear_probe();
        runtime_token.cancel();
        tracker.close();
        let _ = timeout(Duration::from_secs(1), tracker.wait()).await;
        if tunnel.remote_bound.load(Ordering::Acquire) && !shared.closed.is_cancelled() {
            match timeout(
                CHANNEL_TIMEOUT,
                handle.cancel_tcpip_forward(&config.bind_host, config.bind_port as u32),
            )
            .await
            {
                Ok(Ok(())) => {
                    tunnel.remote_bound.store(false, Ordering::Release);
                }
                result => {
                    let detail = match result {
                        Ok(Err(error)) => error.to_string(),
                        _ => "取消远端监听超时".into(),
                    };
                    shared.event(
                        "warning",
                        format!("旧远端监听未确认释放，将关闭 SSH 会话后恢复：{detail}"),
                    );
                    shared.fail("等待旧远端监听释放");
                }
            }
        }
        if token.is_cancelled() || shared.closed.is_cancelled() || !needs_recovery {
            break;
        }
        let delay = (1000u64.saturating_mul(1u64 << retry_attempt.min(5))).min(30_000)
            + (rand::random::<u16>() as u64 % 400);
        retry_attempt += 1;
        shared.state(
            &tunnel,
            json!({"status":"recovering","healthVerified":false,"healthStatus":"pending"}),
        );
        tokio::select! { _ = token.cancelled() => break, _ = shared.closed.cancelled() => break, _ = sleep(Duration::from_millis(delay)) => {} }
    }
    tunnel.active.store(false, Ordering::Release);
    tunnel.clear_probe();
    shared.state(
        &tunnel,
        json!({"status":"stopped","healthVerified":false,"healthStatus":"pending"}),
    );
}

struct Health {
    status: &'static str,
    error: Option<String>,
    verified: bool,
    health_status: &'static str,
    recover: bool,
}

async fn check_health(
    handle: &Arc<client::Handle<Handler>>,
    tunnel: &Tunnel,
    config: &TunnelConfig,
) -> Health {
    if config.direction == "local" {
        return match open_forward(
            handle,
            &config.target_host,
            config.target_port,
            "127.0.0.1",
            0,
        )
        .await
        {
            Ok(channel) => {
                let _ = channel.close().await;
                Health {
                    status: "running",
                    error: Some("TCP 检测通过；未检查应用响应".into()),
                    verified: true,
                    health_status: "tcp-ok",
                    recover: false,
                }
            }
            Err(error) => Health {
                status: "degraded",
                error: Some(format!("目标不可用：{}", error.message)),
                verified: false,
                health_status: "target-unavailable",
                recover: false,
            },
        };
    }
    match timeout(
        CHANNEL_TIMEOUT,
        TcpStream::connect((config.target_host.as_str(), config.target_port)),
    )
    .await
    {
        Ok(Ok(socket)) => drop(socket),
        result => {
            let error = match result {
                Ok(Err(error)) => error.to_string(),
                _ => "本机目标连接超时".into(),
            };
            return Health {
                status: "degraded",
                error: Some(format!("目标不可用：{error}")),
                verified: false,
                health_status: "target-unavailable",
                recover: false,
            };
        }
    }
    let mut challenge = vec![0, 255];
    challenge.extend_from_slice(&rand::random::<[u8; 32]>());
    let mut ack = vec![255, 0];
    ack.extend_from_slice(&rand::random::<[u8; 32]>());
    let probe = Arc::new(Probe {
        challenge,
        ack,
        active: AtomicBool::new(true),
        target_error: Mutex::new(None),
    });
    *tunnel.probe.lock().unwrap() = Some(probe.clone());
    let probe_host = match config.bind_host.as_str() {
        "" | "0.0.0.0" | "localhost" => "127.0.0.1",
        "::" => "::1",
        other => other,
    };
    let result = match open_forward(handle, probe_host, config.bind_port, "127.0.0.1", 0).await {
        Ok(channel) => {
            let mut stream = channel.into_stream();
            let verified = timeout(CHANNEL_TIMEOUT, async {
                stream
                    .write_all(&probe.challenge)
                    .await
                    .map_err(|e| e.to_string())?;
                let mut response = Vec::new();
                let mut bytes = [0u8; 4096];
                loop {
                    let count = stream.read(&mut bytes).await.map_err(|e| e.to_string())?;
                    if count == 0 {
                        return Err("反向转发路径未返回确认，通道已关闭".into());
                    }
                    response.extend_from_slice(&bytes[..count]);
                    if response
                        .windows(probe.ack.len())
                        .any(|window| window == probe.ack)
                    {
                        return Ok::<(), String>(());
                    }
                    if response.len() > 65536 {
                        return Err("反向转发路径响应不匹配".into());
                    }
                }
            })
            .await;
            match verified {
                Ok(Ok(())) => Health {
                    status: "running",
                    error: Some(
                        "TCP 检测通过；反向转发路径及本机目标已验证；未检查应用响应".into(),
                    ),
                    verified: true,
                    health_status: "tcp-ok",
                    recover: false,
                },
                failure => {
                    let target_error = probe.target_error.lock().unwrap().clone();
                    let error = target_error.clone().unwrap_or_else(|| match failure {
                        Ok(Err(error)) => error,
                        _ => "反向转发路径未返回确认，检测超时".into(),
                    });
                    Health {
                        status: if target_error.is_some() {
                            "degraded"
                        } else {
                            "recovering"
                        },
                        error: Some(error),
                        verified: false,
                        health_status: if target_error.is_some() {
                            "target-unavailable"
                        } else {
                            "pending"
                        },
                        recover: target_error.is_none(),
                    }
                }
            }
        }
        Err(error) => Health {
            status: if error.fault { "recovering" } else { "running" },
            error: Some(format!(
                "{}：{}",
                if error.restricted {
                    "服务器限制远端探测"
                } else {
                    "远端探测未验证"
                },
                error.message
            )),
            verified: false,
            health_status: if error.restricted {
                "probe-restricted"
            } else {
                "unverified"
            },
            recover: error.fault,
        },
    };
    tunnel.clear_probe();
    result
}

async fn bridge(
    mut socket: TcpStream,
    channel: Channel<client::Msg>,
    token: CancellationToken,
) -> io::Result<()> {
    let mut stream = channel.into_stream();
    tokio::select! {
        _ = token.cancelled() => Ok(()),
        result = tokio::io::copy_bidirectional(&mut socket, &mut stream) => result.map(|_|()),
    }
}

async fn bridge_remote(
    mut socket: TcpStream,
    channel: Channel<client::Msg>,
    probe: Option<Arc<Probe>>,
    token: CancellationToken,
) -> io::Result<()> {
    let mut stream = channel.into_stream();
    if let Some(probe) = probe {
        // Buffer only a matching nonce prefix. Ordinary traffic is released
        // byte-for-byte at the first mismatch. A server-first protocol still
        // receives its local greeting while this gate is active.
        let mut prefix = Vec::with_capacity(probe.challenge.len());
        let mut from_remote = [0u8; 4096];
        let mut from_local = [0u8; 4096];
        loop {
            tokio::select! {
                _ = token.cancelled() => return Ok(()),
                _ = sleep(Duration::from_millis(100)), if !probe.active.load(Ordering::Acquire) => {
                    if !prefix.is_empty() { return Ok(()); }
                    break;
                },
                read = socket.read(&mut from_local) => {
                    let count = read?;
                    if count == 0 { stream.shutdown().await?; } else { stream.write_all(&from_local[..count]).await?; }
                    if count == 0 { break; }
                },
                read = stream.read(&mut from_remote) => {
                    let count = read?;
                    if count == 0 { socket.shutdown().await?; return Ok(()); }
                    prefix.extend_from_slice(&from_remote[..count]);
                    let match_len = prefix.len().min(probe.challenge.len());
                    if !probe.active.load(Ordering::Acquire) || prefix[..match_len] != probe.challenge[..match_len] {
                        socket.write_all(&prefix).await?;
                        break;
                    }
                    if prefix.len() >= probe.challenge.len() {
                        if probe.active.load(Ordering::Acquire) {
                            stream.write_all(&probe.ack).await?;
                            stream.shutdown().await?;
                        }
                        return Ok(());
                    }
                }
            }
        }
    }
    tokio::select! {
        _ = token.cancelled() => Ok(()),
        result = tokio::io::copy_bidirectional(&mut socket, &mut stream) => result.map(|_|()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
    use russh::server;

    #[derive(Default)]
    struct InputFixtureState {
        opened: AtomicU64,
        closed: AtomicU64,
        eof: AtomicU64,
        active: Mutex<HashSet<russh::ChannelId>>,
    }

    struct InputFixtureSession {
        observed: Arc<InputFixtureState>,
        inputs: HashMap<russh::ChannelId, (Vec<u8>, Sha256, u64)>,
    }

    impl server::Handler for InputFixtureSession {
        type Error = russh::Error;

        async fn auth_password(
            &mut self,
            user: &str,
            password: &str,
        ) -> Result<server::Auth, Self::Error> {
            Ok(if user == "fixture" && password == "fixture-password" {
                server::Auth::Accept
            } else {
                server::Auth::reject()
            })
        }

        async fn channel_open_session(
            &mut self,
            channel: Channel<server::Msg>,
            reply: server::ChannelOpenHandle,
            _session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            self.observed.active.lock().unwrap().insert(channel.id());
            self.observed.opened.fetch_add(1, Ordering::Release);
            reply.accept().await;
            Ok(())
        }

        async fn exec_request(
            &mut self,
            channel: russh::ChannelId,
            command: &[u8],
            session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            session.channel_success(channel)?;
            self.inputs
                .insert(channel, (command.to_vec(), Sha256::new(), 0));
            if command == b"input-early-error" {
                session.extended_data(channel, 1, b"fixture-receive-failed".to_vec())?;
                session.exit_status_request(channel, 69)?;
                session.eof(channel)?;
            } else if command == b"input-early-eof" {
                session.eof(channel)?;
            }
            if command == b"input-duplex" || command == b"input-overflow" {
                // More than the client's default 2 MiB receive window;
                // sending must continue while the client uploads its file.
                session.data(channel, vec![b'x'; 3 * 1024 * 1024])?;
            }
            Ok(())
        }

        async fn data(
            &mut self,
            channel: russh::ChannelId,
            data: &[u8],
            _session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            if let Some((_, hash, count)) = self.inputs.get_mut(&channel) {
                hash.update(data);
                *count += data.len() as u64;
            }
            Ok(())
        }

        async fn channel_eof(
            &mut self,
            channel: russh::ChannelId,
            session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            self.observed.eof.fetch_add(1, Ordering::Release);
            if let Some((command, hash, count)) = self.inputs.remove(&channel) {
                if command == b"input-hang" || command == b"input-overflow" {
                    return Ok(());
                }
                let handle = session.handle();
                tokio::spawn(async move {
                    let reply = format!("\n{count}:{:x}", hash.finalize());
                    let _ = handle.data(channel, reply.into_bytes()).await;
                    let mut stderr = vec![b'z'; 10_000];
                    stderr.extend_from_slice("测试".as_bytes());
                    let _ = handle.extended_data(channel, 1, stderr).await;
                    let _ = handle.exit_status_request(channel, 0).await;
                    let _ = handle.eof(channel).await;
                    let _ = handle.close(channel).await;
                });
            }
            Ok(())
        }

        async fn channel_close(
            &mut self,
            channel: russh::ChannelId,
            _session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            self.inputs.remove(&channel);
            self.observed.active.lock().unwrap().remove(&channel);
            self.observed.closed.fetch_add(1, Ordering::Release);
            Ok(())
        }
    }

    async fn input_fixture() -> (Arc<Connection>, Arc<InputFixtureState>, CancellationToken) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let key = russh::keys::PrivateKey::new(
            KeypairData::Ed25519(Ed25519Keypair::from_seed(&[48u8; 32])),
            "input-fixture",
        )
        .unwrap();
        let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
        let config = Arc::new(server::Config {
            keys: vec![key],
            // Force hundreds of receive-window updates during the upload.
            window_size: 8192,
            maximum_packet_size: 4096,
            auth_rejection_time: Duration::from_millis(1),
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });
        let observed = Arc::new(InputFixtureState::default());
        let state = observed.clone();
        let token = CancellationToken::new();
        let cancelled = token.clone();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let fixture = InputFixtureSession {
                observed: state,
                inputs: HashMap::new(),
            };
            if let Ok(session) =
                server::run_stream(config, CancelIo::new(socket, cancelled.clone()), fixture).await
            {
                tokio::select! { _ = cancelled.cancelled() => {}, _ = session => {} }
            }
        });
        let connection = fixture_connection(port, fingerprint, vec![]).await;
        (connection, observed, token)
    }

    async fn input_file(bytes: &[u8]) -> (tempfile::NamedTempFile, tokio::fs::File) {
        let file = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(file.path(), bytes).await.unwrap();
        let input = tokio::fs::File::open(file.path()).await.unwrap();
        (file, input)
    }

    async fn wait_input_channels_closed(state: &InputFixtureState) {
        timeout(Duration::from_secs(2), async {
            loop {
                if state.active.lock().unwrap().is_empty() {
                    return;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn loopback_exec_input_streams_binary_beyond_windows_and_sends_eof() {
        let (connection, observed, server) = input_fixture().await;
        let data: Vec<u8> = (0..8 * 1024 * 1024).map(|i| (i % 256) as u8).collect();
        let (_file, input) = input_file(&data).await;
        let expected = format!("\n{}:{:x}", data.len(), Sha256::digest(&data));
        let output = connection
            .exec_with_input(
                "input-duplex",
                Duration::from_secs(20),
                4 * 1024 * 1024,
                input,
                data.len() as u64,
            )
            .await
            .unwrap();
        assert_eq!(output.stdout.len(), 3 * 1024 * 1024 + expected.len());
        assert!(output.stdout.ends_with(&expected));
        assert_eq!(output.stderr.chars().count(), 2000);
        assert!(output.stderr.ends_with("测试"));
        assert_eq!(output.exit_code, Some(0));
        assert_eq!(observed.eof.load(Ordering::Acquire), 1);
        connection.close().await;
        server.cancel();
    }

    #[tokio::test]
    async fn loopback_exec_input_checks_size_before_open_and_closes_short_input() {
        use tokio::io::AsyncSeekExt;
        let (connection, observed, server) = input_fixture().await;
        let (_file, input) = input_file(b"binary\0input").await;
        assert!(connection
            .exec_with_input("input-ok", Duration::from_secs(2), 4096, input, 1)
            .await
            .unwrap_err()
            .contains("长度"));
        assert_eq!(observed.opened.load(Ordering::Acquire), 0);
        let (_file, mut input) = input_file(b"binary\0input").await;
        input.seek(std::io::SeekFrom::Start(4)).await.unwrap();
        assert!(connection
            .exec_with_input("input-ok", Duration::from_secs(2), 4096, input, 12)
            .await
            .unwrap_err()
            .contains("缩短"));
        wait_input_channels_closed(&observed).await;
        assert_eq!(observed.closed.load(Ordering::Acquire), 1);
        assert_eq!(observed.eof.load(Ordering::Acquire), 0);
        connection.close().await;
        server.cancel();
    }

    #[tokio::test]
    async fn loopback_exec_input_closes_timeout_overflow_and_dropped_future() {
        let (connection, observed, server) = input_fixture().await;
        let (_file, input) = input_file(b"payload").await;
        assert!(connection
            .exec_with_input("input-hang", Duration::from_millis(100), 4096, input, 7)
            .await
            .unwrap_err()
            .contains("超时"));
        wait_input_channels_closed(&observed).await;
        let (_file, input) = input_file(&vec![0x5a; 1024 * 1024]).await;
        assert!(connection
            .exec_with_input(
                "input-overflow",
                Duration::from_secs(3),
                100,
                input,
                1024 * 1024
            )
            .await
            .unwrap_err()
            .contains("超过限制"));
        wait_input_channels_closed(&observed).await;
        let (_file, input) = input_file(b"payload").await;
        let eofs_before_abort = observed.eof.load(Ordering::Acquire);
        let task_connection = connection.clone();
        let pending = tokio::spawn(async move {
            task_connection
                .exec_with_input("input-hang", Duration::from_secs(30), 4096, input, 7)
                .await
        });
        timeout(Duration::from_secs(2), async {
            while observed.eof.load(Ordering::Acquire) == eofs_before_abort {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        pending.abort();
        let _ = pending.await;
        wait_input_channels_closed(&observed).await;
        assert_eq!(observed.opened.load(Ordering::Acquire), 3);
        assert_eq!(observed.closed.load(Ordering::Acquire), 3);
        // Timeout/abort/overflow affect their channel, not the authenticated connection.
        let (_file, input) = input_file(b"payload").await;
        assert_eq!(
            connection
                .exec_with_input("input-ok", Duration::from_secs(2), 4096, input, 7)
                .await
                .unwrap()
                .exit_code,
            Some(0)
        );
        connection.close().await;
        server.cancel();
    }

    #[tokio::test]
    async fn loopback_exec_input_stops_upload_on_early_remote_eof_or_failure() {
        let (connection, observed, server) = input_fixture().await;
        let data = vec![0x5a; 8 * 1024 * 1024];
        for (command, message) in [
            ("input-early-eof", "结束输出"),
            ("input-early-error", "fixture-receive-failed"),
        ] {
            let (_file, input) = input_file(&data).await;
            let error = connection
                .exec_with_input(
                    command,
                    Duration::from_secs(3),
                    4096,
                    input,
                    data.len() as u64,
                )
                .await
                .unwrap_err();
            assert!(error.contains(message), "{error}");
            wait_input_channels_closed(&observed).await;
        }
        assert_eq!(observed.eof.load(Ordering::Acquire), 0);
        assert_eq!(observed.closed.load(Ordering::Acquire), 2);
        connection.close().await;
        server.cancel();
    }

    struct FixtureSession {
        forwards: HashMap<u32, (CancellationToken, JoinHandle<()>)>,
        deny_probes: bool,
    }

    impl Drop for FixtureSession {
        fn drop(&mut self) {
            for (token, task) in self.forwards.values() {
                token.cancel();
                task.abort();
            }
        }
    }

    impl server::Handler for FixtureSession {
        type Error = russh::Error;

        async fn auth_password(
            &mut self,
            user: &str,
            password: &str,
        ) -> Result<server::Auth, Self::Error> {
            Ok(if user == "fixture" && password == "fixture-password" {
                server::Auth::Accept
            } else {
                server::Auth::reject()
            })
        }

        async fn auth_publickey(
            &mut self,
            user: &str,
            _key: &russh::keys::PublicKey,
        ) -> Result<server::Auth, Self::Error> {
            Ok(if user == "fixture" {
                server::Auth::Accept
            } else {
                server::Auth::reject()
            })
        }

        async fn channel_open_session(
            &mut self,
            _channel: Channel<server::Msg>,
            reply: server::ChannelOpenHandle,
            _session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            Ok(())
        }

        async fn exec_request(
            &mut self,
            channel: russh::ChannelId,
            command: &[u8],
            session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            session.channel_success(channel)?;
            if command == b"hang" {
                return Ok(());
            }
            let handle = session.handle();
            let command = command.to_vec();
            tokio::spawn(async move {
                if command == b"overflow" {
                    let _ = handle.data(channel, vec![b'x'; 4096]).await;
                } else {
                    // Split a multi-byte character across actual SSH packets.
                    let _ = handle.data(channel, vec![0xe6]).await;
                    let _ = handle
                        .data(channel, vec![0xb5, 0x8b, 0xe8, 0xaf, 0x95])
                        .await;
                    let _ = handle
                        .extended_data(channel, 1, b"fixture-stderr".to_vec())
                        .await;
                }
                let _ = handle
                    .exit_status_request(channel, if command == b"nonzero" { 7 } else { 0 })
                    .await;
                let _ = handle.eof(channel).await;
                let _ = handle.close(channel).await;
            });
            Ok(())
        }

        async fn channel_open_direct_tcpip(
            &mut self,
            channel: Channel<server::Msg>,
            host: &str,
            port: u32,
            _origin_host: &str,
            _origin_port: u32,
            reply: server::ChannelOpenHandle,
            _session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            if self.deny_probes && self.forwards.contains_key(&port) {
                reply
                    .reject(ChannelOpenFailure::AdministrativelyProhibited)
                    .await;
                return Ok(());
            }
            let host = host.to_owned();
            tokio::spawn(async move {
                match TcpStream::connect((host.as_str(), port as u16)).await {
                    Ok(mut socket) => {
                        reply.accept().await;
                        let mut stream = channel.into_stream();
                        let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
                    }
                    Err(_) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
                }
            });
            Ok(())
        }

        async fn tcpip_forward(
            &mut self,
            address: &str,
            port: &mut u32,
            session: &mut server::Session,
        ) -> Result<bool, Self::Error> {
            let Ok(listener) = TcpListener::bind((address, *port as u16)).await else {
                return Ok(false);
            };
            *port = listener.local_addr()?.port() as u32;
            let bind_port = *port;
            let address = address.to_owned();
            let handle = session.handle();
            let token = CancellationToken::new();
            let cancelled = token.clone();
            let task = tokio::spawn(async move {
                loop {
                    let accepted = tokio::select! {_ = cancelled.cancelled()=>break,result=listener.accept()=>result};
                    let Ok((mut socket, origin)) = accepted else {
                        break;
                    };
                    let handle = handle.clone();
                    let address = address.clone();
                    let token = cancelled.clone();
                    tokio::spawn(async move {
                        let channel = tokio::select! {
                            _ = token.cancelled()=>return,
                            channel = handle.channel_open_forwarded_tcpip(address,bind_port,origin.ip().to_string(),origin.port() as u32)=>channel,
                        };
                        if let Ok(channel) = channel {
                            let mut stream = channel.into_stream();
                            tokio::select! {_ = token.cancelled()=>{},_ = tokio::io::copy_bidirectional(&mut socket,&mut stream)=>{}}
                        }
                    });
                }
            });
            self.forwards.insert(*port, (token, task));
            Ok(true)
        }

        async fn cancel_tcpip_forward(
            &mut self,
            _address: &str,
            port: u32,
            _session: &mut server::Session,
        ) -> Result<bool, Self::Error> {
            if let Some((token, task)) = self.forwards.remove(&port) {
                token.cancel();
                let _ = task.await;
                Ok(true)
            } else {
                Ok(false)
            }
        }
    }

    async fn fixture(deny_probes: bool) -> (u16, String, CancellationToken) {
        fixture_with_preferred(deny_probes, russh::Preferred::default()).await
    }

    async fn fixture_with_preferred(
        deny_probes: bool,
        preferred: russh::Preferred,
    ) -> (u16, String, CancellationToken) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let key = russh::keys::PrivateKey::new(
            KeypairData::Ed25519(Ed25519Keypair::from_seed(&[47u8; 32])),
            "fixture",
        )
        .unwrap();
        let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
        let config = Arc::new(server::Config {
            keys: vec![key],
            preferred,
            auth_rejection_time: Duration::from_millis(1),
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });
        let token = CancellationToken::new();
        let cancelled = token.clone();
        tokio::spawn(async move {
            let tracker = TaskTracker::new();
            loop {
                let accept = tokio::select! {_ = cancelled.cancelled()=>break,result=listener.accept()=>result};
                let Ok((socket, _)) = accept else { break };
                let config = config.clone();
                let token = cancelled.clone();
                tracker.spawn(async move {
                    let fixture = FixtureSession {
                        forwards: HashMap::new(),
                        deny_probes,
                    };
                    if let Ok(session) =
                        server::run_stream(config, CancelIo::new(socket, token.clone()), fixture)
                            .await
                    {
                        tokio::select! {_ = token.cancelled()=>{},_ = session=>{}}
                    }
                });
            }
            tracker.close();
            let _ = timeout(Duration::from_secs(1), tracker.wait()).await;
        });
        (port, fingerprint, token)
    }

    fn fixture_profile(port: u16, tunnels: Vec<Value>) -> Value {
        json!({"id":"fixture","host":"127.0.0.1","port":port,"username":"fixture","authType":"password","tunnels":tunnels})
    }

    fn sink() -> EventSink {
        Arc::new(|_, _| {})
    }

    async fn fixture_connection(
        port: u16,
        fingerprint: String,
        tunnels: Vec<Value>,
    ) -> Arc<Connection> {
        let (_trust, receiver) = trust_channel();
        timeout(
            Duration::from_secs(5),
            Connection::connect(
                fixture_profile(port, tunnels),
                json!({"password":"fixture-password"}),
                vec![fingerprint],
                sink(),
                receiver,
            ),
        )
        .await
        .unwrap()
        .unwrap()
    }

    async fn unused_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    }

    async fn wait_health(connection: &Connection, id: &str, status: &str) -> Value {
        timeout(Duration::from_secs(6), async {
            loop {
                if let Some(state) = connection
                    .tunnel_states()
                    .into_iter()
                    .find(|state| state["id"] == id && state["healthStatus"] == status)
                {
                    return state;
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap()
    }

    async fn echo_server() -> (u16, CancellationToken) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = CancellationToken::new();
        let cancel = token.clone();
        tokio::spawn(async move {
            loop {
                let accepted =
                    tokio::select! {_ = cancel.cancelled()=>break,result=listener.accept()=>result};
                if let Ok((mut socket, _)) = accepted {
                    tokio::spawn(async move {
                        let mut data = Vec::new();
                        if socket.read_to_end(&mut data).await.is_ok() {
                            let _ = socket.write_all(&data).await;
                            let _ = socket.shutdown().await;
                        }
                    });
                }
            }
        });
        (port, token)
    }

    #[test]
    fn validates_tunnel_ports_ids_and_health_interval() {
        assert!(TunnelConfig::parse(
            json!({"id":"x","direction":"local","bindPort":0,"targetPort":22})
        )
        .is_err());
        let rule = json!({"id":"x","direction":"local","bindPort":"1234","targetPort":22,"healthIntervalSec":1});
        assert_eq!(
            TunnelConfig::parse(rule.clone())
                .unwrap()
                .health_interval_sec,
            2
        );
        assert!(parse_rules(vec![rule.clone(), rule]).is_err());
    }

    #[tokio::test]
    async fn cancellation_interrupts_an_idle_tcp_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (_peer, _) = listener.accept().await.unwrap();
        let cancel = CancellationToken::new();
        let mut stream = CancelIo::new(socket, cancel.clone());
        let task = tokio::spawn(async move {
            let mut b = [0u8; 1];
            stream.read(&mut b).await
        });
        cancel.cancel();
        assert!(timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .is_err());
    }

    #[tokio::test]
    async fn loopback_exec_preserves_utf8_stderr_nonzero_and_closes_timeout() {
        let (port, fingerprint, server) = fixture(false).await;
        let connection = fixture_connection(port, fingerprint, vec![]).await;
        let output = connection
            .exec("nonzero", Duration::from_secs(2), 4096)
            .await
            .unwrap();
        assert_eq!(output.stdout, "测试");
        assert_eq!(output.stderr, "fixture-stderr");
        assert_eq!(output.exit_code, Some(7));
        assert!(connection
            .exec("overflow", Duration::from_secs(2), 100)
            .await
            .unwrap_err()
            .contains("超过限制"));
        assert!(connection
            .exec("hang", Duration::from_millis(100), 4096)
            .await
            .unwrap_err()
            .contains("超时"));
        // A timed out channel cannot block the next independent command.
        assert_eq!(
            connection
                .exec("ok", Duration::from_secs(2), 4096)
                .await
                .unwrap()
                .exit_code,
            Some(0)
        );
        connection.close().await;
        assert!(connection.is_closed());
        server.cancel();
    }

    #[tokio::test]
    async fn loopback_requires_matching_host_decision_and_rejects_changed_key() {
        let (port, fingerprint, server) = fixture(false).await;
        let (trust, receiver) = trust_channel();
        let decisions = trust.clone();
        let expected = fingerprint.clone();
        let events: EventSink = Arc::new(move |_, event| {
            if event["type"] == "host-key" {
                assert_eq!(event["fingerprint"], expected);
                assert_eq!(event["changed"], true);
                decisions
                    .send(HostDecision {
                        fingerprint: "wrong fingerprint".into(),
                        accepted: true,
                    })
                    .unwrap();
                decisions
                    .send(HostDecision {
                        fingerprint: expected.clone(),
                        accepted: false,
                    })
                    .unwrap();
            }
        });
        let result = timeout(
            Duration::from_secs(5),
            Connection::connect(
                fixture_profile(port, vec![]),
                json!({"password":"fixture-password"}),
                vec!["SHA256:old-key".into()],
                events,
                receiver,
            ),
        )
        .await
        .unwrap();
        assert!(result.err().unwrap().contains("拒绝服务器指纹"));
        let (trust, receiver) = trust_channel();
        trust
            .send(HostDecision {
                fingerprint,
                accepted: true,
            })
            .unwrap();
        let connection = timeout(
            Duration::from_secs(5),
            Connection::connect(
                fixture_profile(port, vec![]),
                json!({"password":"fixture-password"}),
                vec![],
                sink(),
                receiver,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        connection.close().await;
        server.cancel();
    }

    #[tokio::test]
    async fn loopback_local_and_remote_tunnels_preserve_half_close_and_release_ports() {
        let (port, fingerprint, server) = fixture(false).await;
        let (target, target_cancel) = echo_server().await;
        let local_port = unused_port().await;
        let remote_port = unused_port().await;
        let rule = |id: &str, direction: &str, bind_port: u16| json!({"id":id,"name":id,"direction":direction,"bindHost":"127.0.0.1","bindPort":bind_port,"targetHost":"127.0.0.1","targetPort":target,"healthIntervalSec":2});
        let connection = fixture_connection(
            port,
            fingerprint,
            vec![
                rule("local", "local", local_port),
                rule("remote", "remote", remote_port),
            ],
        )
        .await;
        assert_eq!(
            wait_health(&connection, "local", "tcp-ok").await["healthVerified"],
            true
        );
        assert_eq!(
            wait_health(&connection, "remote", "tcp-ok").await["healthVerified"],
            true
        );
        for forwarded_port in [local_port, remote_port] {
            let data = vec![0x5au8; 300_000];
            let mut socket = TcpStream::connect(("127.0.0.1", forwarded_port))
                .await
                .unwrap();
            socket.write_all(&data).await.unwrap();
            socket.shutdown().await.unwrap();
            let mut response = Vec::new();
            timeout(Duration::from_secs(5), socket.read_to_end(&mut response))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(response, data);
        }
        connection.set_tunnel("local", false).await.unwrap();
        let listener = TcpListener::bind(("127.0.0.1", local_port)).await.unwrap();
        drop(listener);
        connection.restart_tunnel("remote").await.unwrap();
        wait_health(&connection, "remote", "tcp-ok").await;
        connection.close().await;
        let listener = TcpListener::bind(("127.0.0.1", remote_port)).await.unwrap();
        drop(listener);
        target_cancel.cancel();
        server.cancel();
    }

    #[tokio::test]
    async fn loopback_reverse_health_marks_administrative_probe_restriction_unverified() {
        let (port, fingerprint, server) = fixture(true).await;
        let (target, target_cancel) = echo_server().await;
        let bind_port = unused_port().await;
        let rule =
            json!({"id":"remote","direction":"remote","bindPort":bind_port,"targetPort":target});
        let connection = fixture_connection(port, fingerprint, vec![rule]).await;
        let state = wait_health(&connection, "remote", "probe-restricted").await;
        assert_eq!(state["status"], "running");
        assert_eq!(state["healthVerified"], false);
        connection.close().await;
        target_cancel.cancel();
        server.cancel();
    }

    #[test]
    fn compatibility_algorithms_keep_modern_priority_without_enabling_sha1_kex() {
        let defaults = russh::Preferred::default();
        let preferred = compatible_preferred();
        assert!(preferred.mac.starts_with(&defaults.mac));
        assert!(preferred.cipher.starts_with(&defaults.cipher));
        assert!(preferred.mac.contains(&russh::mac::HMAC_SHA1_ETM));
        assert!(preferred.mac.contains(&russh::mac::HMAC_SHA1));
        assert!(preferred.cipher.contains(&russh::cipher::AES_128_GCM));
        assert!(!preferred.kex.iter().any(|name| {
            name.as_ref().starts_with("diffie-hellman-") && name.as_ref().ends_with("sha1")
        }));
    }

    #[tokio::test]
    async fn loopback_compatibility_supports_all_three_ecdh_and_both_sha1_macs() {
        for kex in [
            russh::kex::ECDH_SHA2_NISTP256,
            russh::kex::ECDH_SHA2_NISTP384,
            russh::kex::ECDH_SHA2_NISTP521,
        ] {
            for mac in [russh::mac::HMAC_SHA1_ETM, russh::mac::HMAC_SHA1] {
                let preferred = russh::Preferred {
                    kex: std::borrow::Cow::Owned(vec![kex]),
                    cipher: std::borrow::Cow::Owned(vec![russh::cipher::AES_128_CTR]),
                    mac: std::borrow::Cow::Owned(vec![mac]),
                    ..Default::default()
                };
                let (port, fingerprint, server) = fixture_with_preferred(false, preferred).await;
                let connection = fixture_connection(port, fingerprint, vec![]).await;
                assert_eq!(
                    connection
                        .exec("ok", Duration::from_secs(2), 4096)
                        .await
                        .unwrap()
                        .stdout,
                    "测试",
                    "ECDH {:?} with MAC {:?} must execute over the encrypted SSH connection",
                    kex,
                    mac,
                );
                connection.close().await;
                server.cancel();
            }
        }
    }

    #[tokio::test]
    async fn loopback_compatibility_supports_aes128_gcm() {
        let preferred = russh::Preferred {
            kex: std::borrow::Cow::Owned(vec![russh::kex::ECDH_SHA2_NISTP256]),
            cipher: std::borrow::Cow::Owned(vec![russh::cipher::AES_128_GCM]),
            ..Default::default()
        };
        let (port, fingerprint, server) = fixture_with_preferred(false, preferred).await;
        let connection = fixture_connection(port, fingerprint, vec![]).await;
        assert_eq!(
            connection
                .exec("ok", Duration::from_secs(2), 4096)
                .await
                .unwrap()
                .exit_code,
            Some(0)
        );
        connection.close().await;
        server.cancel();
    }
}
