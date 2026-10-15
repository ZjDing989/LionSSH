//! Pipe-only integration harness for the same Rust service used by the desktop.
//! An explicit data directory is mandatory; no window, global instance lock or
//! network listener is created by this mode.
use crate::service::Service;
use serde_json::{json, Value};
use std::{
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;

/// Opt-in UI verification has its own single-instance namespace and must keep
/// all application data inside the caller's explicit temporary fixture root.
pub fn desktop_context(
    mut context: tauri::Context<tauri::Wry>,
) -> Result<tauri::Context<tauri::Wry>, String> {
    if !std::env::args().any(|arg| arg == "--verification-instance") {
        return Ok(context);
    }
    let id = uuid::Uuid::parse_str(
        &std::env::var("LION_VERIFY_INSTANCE_ID").map_err(|_| "桌面验证必须指定实例 UUID")?,
    )
    .map_err(|_| "桌面验证实例 UUID 无效")?;
    let root =
        PathBuf::from(std::env::var_os("LION_VERIFY_ROOT").ok_or("桌面验证必须指定隔离目录")?);
    if !root.is_absolute() {
        return Err("桌面验证隔离目录必须是绝对路径".into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let directory = crate::service::data_directory()?;
    if directory == root || !directory.starts_with(&root) {
        return Err("桌面验证数据必须位于隔离目录内".into());
    }
    context
        .config_mut()
        .identifier
        .push_str(&format!(".verification.{}", id.simple()));
    Ok(context)
}

pub async fn run() -> Result<(), String> {
    let directory =
        PathBuf::from(std::env::var_os("LION_DATA_DIR").ok_or("后台验证必须指定 LION_DATA_DIR")?);
    if !directory.is_absolute() {
        return Err("后台验证数据目录必须是绝对路径".into());
    }
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let output = Arc::new(Mutex::new(std::io::stdout()));
    let emitter = output.clone();
    let service = Service::open(
        directory,
        "light",
        Arc::new(move |event, payload| {
            if let Ok(mut writer) = emitter.lock() {
                let _ = writeln!(writer, "{}", json!({"event":event,"payload":payload}));
                let _ = writer.flush();
            }
        }),
    );
    let stop = CancellationToken::new();
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    let mut jobs = tokio::task::JoinSet::new();
    loop {
        let line = tokio::select! { _ = stop.cancelled()=>break, value=input.next_line()=>value.map_err(|e|e.to_string())? };
        let Some(line) = line else {
            break;
        };
        if line.len() > 2 * 1024 * 1024 {
            return Err("验证请求过长".into());
        }
        let request: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        let service = service.clone();
        let writer = output.clone();
        let stop = stop.clone();
        jobs.spawn(async move {
            let id = request["id"].clone();
            let method = request["method"].as_str().unwrap_or("");
            let args = request["args"].as_array().cloned().unwrap_or_default();
            let response = match service.request(method, args).await {
                Ok(result) => json!({"id":id,"result":result}),
                Err(error) => json!({"id":id,"error":error}),
            };
            if let Ok(mut output) = writer.lock() {
                let _ = writeln!(output, "{response}");
                let _ = output.flush();
            }
            if method == "shutdown" {
                stop.cancel();
            }
        });
        while jobs.try_join_next().is_some() {}
    }
    service.shutdown().await;
    jobs.abort_all();
    while jobs.join_next().await.is_some() {}
    service.kill();
    Ok(())
}
