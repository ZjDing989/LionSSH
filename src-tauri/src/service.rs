use crate::{
    model,
    monitor::Monitor,
    ssh::{self, Connection, HostDecision},
    store::Store,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::{sync::Mutex as AsyncMutex, task::JoinHandle};
use tokio_util::sync::CancellationToken;

type Sink = Arc<dyn Fn(&str, Value) + Send + Sync>;
type Reply = Result<Value, String>;

pub const PUBLIC_METHODS: &[&str] = &[
    "initial",
    "set-theme",
    "save-profile",
    "delete-profile",
    "connect",
    "disconnect",
    "refresh-gpu",
    "refresh-slurm",
    "refresh-storage",
    "prepare-job-cancel",
    "cancel-job",
    "get-job-priority",
    "prepare-tmp-cleanup",
    "cleanup-tmp",
    "set-slurm-scope",
    "update-gpu",
    "set-tunnel",
    "restart-tunnel",
    "trust-host",
];

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn system_theme(fallback: &str) -> String {
    #[cfg(windows)]
    {
        use winreg::{enums::HKEY_CURRENT_USER, RegKey};
        if let Ok(value) = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
            .and_then(|key| key.get_value::<u32, _>("AppsUseLightTheme"))
        {
            return if value == 0 { "dark" } else { "light" }.into();
        }
    }
    fallback.into()
}

pub fn data_directory() -> Result<PathBuf, String> {
    let explicit = std::env::var_os("LION_DATA_DIR")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let executable = if cfg!(windows) && !cfg!(debug_assertions) && explicit.is_none() {
        Some(std::env::current_exe().map_err(|error| error.to_string())?)
    } else {
        None
    };
    #[cfg(windows)]
    let platform_base: Option<PathBuf> = None;
    #[cfg(not(windows))]
    let platform_base = dirs::data_dir();
    let directory = select_data_directory(
        explicit,
        cfg!(windows),
        cfg!(debug_assertions),
        Path::new(env!("CARGO_MANIFEST_DIR")),
        executable.as_deref(),
        platform_base.as_deref(),
    )?;
    #[cfg(unix)]
    let created = {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)
    };
    #[cfg(not(unix))]
    let created = std::fs::create_dir_all(&directory);
    created.map_err(|error| format!("无法创建数据目录 {}：{error}", directory.display()))?;
    directory.canonicalize().map_err(|e| e.to_string())
}

fn select_data_directory(
    explicit: Option<PathBuf>,
    windows: bool,
    development: bool,
    manifest: &Path,
    executable: Option<&Path>,
    platform_base: Option<&Path>,
) -> Result<PathBuf, String> {
    // Keep the existing override semantics, including relative paths resolved
    // against the process working directory by the final canonicalize call.
    if let Some(directory) = explicit {
        return Ok(directory);
    }
    if windows {
        let parent = if development {
            manifest.parent()
        } else {
            executable.and_then(Path::parent)
        };
        return parent
            .map(|parent| parent.join("data"))
            .ok_or_else(|| "无法确定应用程序目录".into());
    }
    // macOS app bundles and Linux AppImage mounts may be read-only. dirs uses
    // Application Support on macOS and the XDG data directory on Linux.
    platform_base
        .map(|base| base.join("tools.lion.ssh"))
        .ok_or_else(|| "无法确定用户数据目录，请通过 LION_DATA_DIR 指定可写目录".into())
}

struct Device {
    id: String,
    profile: Mutex<Value>,
    secrets: Mutex<Value>,
    desired: AtomicBool,
    cancel: CancellationToken,
    trust: Mutex<Option<ssh::TrustSender>>,
    connection: Mutex<Option<Arc<Connection>>>,
    monitor: Mutex<Option<Arc<Monitor>>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

pub struct Service {
    directory: PathBuf,
    store: AsyncMutex<Option<Store>>,
    states: Mutex<HashMap<String, Value>>,
    devices: Mutex<HashMap<String, Arc<Device>>>,
    secrets: Mutex<HashMap<String, Value>>,
    host_keys: Mutex<HashMap<String, Value>>,
    operations: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    theme: Mutex<String>,
    theme_preference: Mutex<Option<String>>,
    sink: Sink,
    pub quitting: AtomicBool,
    pub finished: AtomicBool,
    pub tray_available: AtomicBool,
}

fn argument<'a>(args: &'a [Value], index: usize, label: &str) -> Result<&'a Value, String> {
    args.get(index).ok_or_else(|| format!("缺少{label}"))
}
fn string_argument<'a>(args: &'a [Value], index: usize, label: &str) -> Result<&'a str, String> {
    argument(args, index, label)?
        .as_str()
        .ok_or_else(|| format!("{label}格式不正确"))
}
fn clean_secrets(input: Option<&Value>) -> Reply {
    let mut result = json!({});
    if let Some(input) = input.filter(|v| !v.is_null()) {
        let object = input.as_object().ok_or("凭据格式不正确")?;
        for key in ["password", "passphrase"] {
            if let Some(value) = object.get(key) {
                let value = value.as_str().ok_or("凭据格式不正确")?;
                if value.encode_utf16().count() > 4096 {
                    return Err("凭据内容过长".into());
                }
                if !value.is_empty() {
                    result[key] = json!(value);
                }
            }
        }
    }
    Ok(result)
}
fn merge(target: &mut Value, values: &Value) {
    if let (Some(target), Some(values)) = (target.as_object_mut(), values.as_object()) {
        for (key, value) in values {
            target.insert(key.clone(), value.clone());
        }
    }
}

impl Service {
    pub fn spawn(app: &AppHandle, directory: &Path, resolved: &str) -> Result<Arc<Self>, String> {
        let app = app.clone();
        let sink: Sink = Arc::new(move |event, payload| {
            if event == "lion:theme" {
                apply_theme(&app, &payload);
            }
            let _ = app.emit(event, payload);
        });
        Ok(Self::open(directory.to_path_buf(), resolved, sink))
    }

    pub fn open(directory: PathBuf, resolved: &str, sink: Sink) -> Arc<Self> {
        let store = Store::open(directory.clone()).ok();
        let states = store
            .as_ref()
            .map(|store| {
                store
                    .profiles
                    .iter()
                    .filter_map(|profile| {
                        Some((
                            profile["id"].as_str()?.to_string(),
                            model::blank_state(profile),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Arc::new(Self {
            directory,
            store: AsyncMutex::new(store),
            states: Mutex::new(states),
            devices: Mutex::new(HashMap::new()),
            secrets: Mutex::new(HashMap::new()),
            host_keys: Mutex::new(HashMap::new()),
            operations: Mutex::new(HashMap::new()),
            theme: Mutex::new(resolved.into()),
            theme_preference: Mutex::new(None),
            sink,
            quitting: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            tray_available: AtomicBool::new(false),
        })
    }

    async fn ensure_store(&self) -> Result<(), String> {
        let mut guard = self.store.lock().await;
        if guard.is_none() {
            let store = Store::open(self.directory.clone())?;
            let mut states = self.states.lock().unwrap();
            for profile in &store.profiles {
                if let Some(id) = profile["id"].as_str() {
                    states.insert(id.into(), model::blank_state(profile));
                }
            }
            *guard = Some(store);
        }
        Ok(())
    }
    async fn profile(&self, id: &str) -> Reply {
        self.store
            .lock()
            .await
            .as_ref()
            .ok_or("配置未就绪")?
            .get(id)
    }
    fn update_state(&self, id: &str, change: impl FnOnce(&mut Value)) {
        let value = {
            let mut states = self.states.lock().unwrap();
            let Some(state) = states.get_mut(id) else {
                return;
            };
            change(state);
            state.clone()
        };
        (self.sink)("lion:state", value);
    }
    fn status(&self, id: &str, status: &str, error: Option<&str>) {
        if status != "awaiting-trust" {
            self.host_keys.lock().unwrap().remove(id);
        }
        self.update_state(id, |state| {
            state["status"] = json!(status);
            state["error"] = error.map(|v| json!(v)).unwrap_or(Value::Null);
            if status != "connected" && status != "awaiting-trust" && status != "connecting" {
                for kind in ["gpu", "slurm", "storage"] {
                    state[kind]["status"] = json!("unknown");
                    state[kind]["nextPollAt"] = Value::Null;
                }
            }
        });
    }
    fn event(&self, id: &str, level: &str, message: &str) {
        self.update_state(id, |state| { let event = json!({"id":uuid::Uuid::new_v4().to_string(), "time":now(), "level":level, "message":message}); if let Some(events) = state["events"].as_array_mut() { events.insert(0,event); events.truncate(100); } });
    }
    fn current(&self, device: &Arc<Device>) -> bool {
        self.devices
            .lock()
            .unwrap()
            .get(&device.id)
            .is_some_and(|current| Arc::ptr_eq(current, device))
            && device.desired.load(Ordering::SeqCst)
            && !self.quitting.load(Ordering::SeqCst)
    }
    fn receive(&self, device: &Arc<Device>, event: &str, payload: Value) {
        if !self.current(device) {
            return;
        }
        match event {
            "gpu" | "slurm" | "storage" => self.update_state(&device.id, |state| {
                state[event] = payload;
            }),
            "event" => self.event(
                &device.id,
                payload["level"].as_str().unwrap_or("info"),
                payload["message"].as_str().unwrap_or(""),
            ),
            "transport" => match payload["type"].as_str().unwrap_or("") {
                "status" => self.status(
                    &device.id,
                    payload["status"].as_str().unwrap_or("connecting"),
                    None,
                ),
                "host-key" => {
                    self.status(&device.id, "awaiting-trust", None);
                    let mut request = payload;
                    request["profileId"] = json!(device.id);
                    self.host_keys
                        .lock()
                        .unwrap()
                        .insert(device.id.clone(), request.clone());
                    (self.sink)("lion:host-key", request);
                }
                "tunnel" => self.update_state(&device.id, |state| {
                    let value = payload["state"].clone();
                    if let Some(tunnels) = state["tunnels"].as_array_mut() {
                        if let Some(old) = tunnels.iter_mut().find(|t| t["id"] == value["id"]) {
                            *old = value;
                        } else {
                            tunnels.push(value);
                        }
                    }
                }),
                "tunnels" => self.update_state(&device.id, |state| {
                    state["tunnels"] = payload["states"].clone();
                }),
                "event" => self.event(
                    &device.id,
                    payload["level"].as_str().unwrap_or("info"),
                    payload["message"].as_str().unwrap_or(""),
                ),
                _ => {}
            },
            _ => {}
        }
    }
    fn sink_for(self: &Arc<Self>, device: &Arc<Device>) -> Sink {
        let owner = Arc::downgrade(self);
        let device = Arc::downgrade(device);
        Arc::new(move |event, payload| {
            if let (Some(owner), Some(device)) = (owner.upgrade(), device.upgrade()) {
                owner.receive(&device, event, payload);
            }
        })
    }

    async fn run_device(self: Arc<Self>, device: Arc<Device>) {
        let mut attempt = 0u32;
        while self.current(&device) && !device.cancel.is_cancelled() {
            let profile = device.profile.lock().unwrap().clone();
            let secrets = device.secrets.lock().unwrap().clone();
            let trusted = {
                let store = self.store.lock().await;
                store
                    .as_ref()
                    .unwrap()
                    .fingerprints(&profile)
                    .unwrap_or_default()
            };
            let (sender, receiver) = ssh::trust_channel();
            *device.trust.lock().unwrap() = Some(sender);
            self.status(
                &device.id,
                if attempt == 0 {
                    "connecting"
                } else {
                    "reconnecting"
                },
                None,
            );
            let connection = tokio::select! { _ = device.cancel.cancelled() => break, result = Connection::connect(profile.clone(),secrets,trusted,self.sink_for(&device),receiver) => result };
            let failure = match connection {
                Ok(connection) => {
                    if !self.current(&device) {
                        connection.close().await;
                        break;
                    }
                    *device.connection.lock().unwrap() = Some(connection.clone());
                    *device.trust.lock().unwrap() = None;
                    self.status(&device.id, "connected", None);
                    self.event(&device.id, "success", "SSH 连接已建立");
                    attempt = 0;
                    let current_profile = device.profile.lock().unwrap().clone();
                    let _ = connection
                        .update_tunnels(
                            current_profile["tunnels"]
                                .as_array()
                                .cloned()
                                .unwrap_or_default(),
                        )
                        .await;
                    let monitor =
                        Monitor::new(current_profile, connection.clone(), self.sink_for(&device));
                    let scope = self
                        .states
                        .lock()
                        .unwrap()
                        .get(&device.id)
                        .and_then(|s| s["slurm"]["scope"].as_str())
                        .unwrap_or("mine")
                        .to_string();
                    if scope == "all" {
                        let _ = monitor.initialize_slurm_scope("all").await;
                    }
                    *device.monitor.lock().unwrap() = Some(monitor.clone());
                    monitor.start();
                    let reason = tokio::select! { _ = device.cancel.cancelled() => "连接已关闭".to_string(), reason = connection.closed() => reason };
                    monitor.stop().await;
                    connection.close().await;
                    device.monitor.lock().unwrap().take();
                    device.connection.lock().unwrap().take();
                    reason
                }
                Err(error) => error,
            };
            self.host_keys.lock().unwrap().remove(&device.id);
            device.trust.lock().unwrap().take();
            if !self.current(&device) || device.cancel.is_cancelled() {
                break;
            }
            self.event(&device.id, "warning", &failure);
            let terminal = [
                "认证失败",
                "私钥",
                "口令",
                "密码",
                "已拒绝",
                "指纹确认超时",
                "authentication",
                "Authentication",
                "key format",
            ]
            .iter()
            .any(|word| failure.contains(word));
            if device.profile.lock().unwrap()["autoReconnect"] == false || terminal {
                self.status(&device.id, "error", Some(&failure));
                device.desired.store(false, Ordering::SeqCst);
                break;
            }
            self.status(&device.id, "reconnecting", Some(&failure));
            let delay = (1000u64 * (1u64 << attempt.min(5))).min(30_000) + (now() % 400);
            attempt += 1;
            self.event(
                &device.id,
                "info",
                &format!("{} 秒后重新连接；隧道规则保留", delay.div_ceil(1000)),
            );
            tokio::select! { _ = device.cancel.cancelled() => break, _ = tokio::time::sleep(Duration::from_millis(delay)) => {} }
        }
    }

    async fn disconnect(&self, id: &str) -> Result<(), String> {
        self.host_keys.lock().unwrap().remove(id);
        let device = self.devices.lock().unwrap().remove(id);
        if let Some(device) = device {
            device.desired.store(false, Ordering::SeqCst);
            device.cancel.cancel();
            device.trust.lock().unwrap().take();
            let task = device.task.lock().unwrap().take();
            if let Some(mut task) = task {
                if tokio::time::timeout(Duration::from_secs(16), &mut task)
                    .await
                    .is_err()
                {
                    task.abort();
                    let _ = task.await;
                }
            }
            let monitor = device.monitor.lock().unwrap().take();
            if let Some(monitor) = monitor {
                monitor.stop().await;
            }
            let connection = device.connection.lock().unwrap().take();
            if let Some(connection) = connection {
                connection.close().await;
            }
        }
        self.status(id, "disconnected", None);
        self.update_state(id, |state| {
            if let Some(tunnels) = state["tunnels"].as_array_mut() {
                for tunnel in tunnels {
                    tunnel["status"] = json!("stopped");
                    tunnel["healthVerified"] = json!(false);
                }
            }
        });
        Ok(())
    }
    fn connected(&self, id: &str) -> Result<Arc<Device>, String> {
        let device = self
            .devices
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("请先连接设备")?;
        if !device.desired.load(Ordering::SeqCst)
            || self
                .states
                .lock()
                .unwrap()
                .get(id)
                .is_none_or(|state| state["status"] != "connected")
        {
            return Err("请先连接设备".into());
        }
        Ok(device)
    }
    async fn credentials(&self, profile: &Value, supplied: Option<&Value>) -> Reply {
        let supplied = clean_secrets(supplied)?;
        let id = profile["id"].as_str().unwrap();
        let cached = self
            .secrets
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or(json!({}));
        let saved = {
            let guard = self.store.lock().await;
            guard.as_ref().unwrap().read_secret(id)
        };
        let relevant = if profile["authType"] == "password" {
            "password"
        } else {
            "passphrase"
        };
        let mut result = match saved {
            Ok(value) => value,
            Err(error)
                if supplied[relevant].as_str().unwrap_or("").is_empty()
                    && cached[relevant].as_str().unwrap_or("").is_empty() =>
            {
                return Err(error)
            }
            Err(_) => json!({}),
        };
        merge(&mut result, &cached);
        merge(&mut result, &supplied);
        if relevant == "password" && result["password"].as_str().unwrap_or("").is_empty() {
            return Err("请先输入 SSH 密码".into());
        }
        if profile["authType"] == "key"
            && !Path::new(profile["keyPath"].as_str().unwrap_or("")).is_file()
        {
            return Err("密钥文件不存在或无法读取，请检查路径".into());
        }
        self.secrets
            .lock()
            .unwrap()
            .insert(id.into(), result.clone());
        Ok(result)
    }
    async fn theme_state(&self) -> Value {
        let preference = self
            .store
            .lock()
            .await
            .as_ref()
            .map(|store| {
                store.settings["theme"]
                    .as_str()
                    .unwrap_or("system")
                    .to_string()
            })
            .unwrap_or("system".into());
        let resolved = if preference == "system" {
            self.theme.lock().unwrap().clone()
        } else {
            preference.clone()
        };
        json!({"preference":preference,"resolved":resolved})
    }

    pub async fn request(self: &Arc<Self>, method: &str, args: Vec<Value>) -> Reply {
        if method == "shutdown" {
            self.shutdown().await;
            return Ok(Value::Null);
        }
        if !PUBLIC_METHODS.contains(&method) && method != "native-theme" {
            return Err("不支持的应用操作".into());
        }
        if self.quitting.load(Ordering::SeqCst) {
            return Err("应用正在关闭".into());
        }
        if args.len() > 4 {
            return Err("调用参数格式不正确".into());
        }
        self.ensure_store().await?;
        if method == "initial" {
            let profiles = self.store.lock().await.as_ref().unwrap().public_profiles();
            let states: Vec<_> = self.states.lock().unwrap().values().cloned().collect();
            let keys: Vec<_> = self.host_keys.lock().unwrap().values().cloned().collect();
            return Ok(
                json!({"profiles":profiles,"states":states,"version":env!("CARGO_PKG_VERSION"),"theme":self.theme_state().await,"shutdown":Value::Null,"pendingHostKeys":keys}),
            );
        }
        if method == "set-theme" || method == "native-theme" {
            let theme = string_argument(&args, 0, "主题")?;
            if method == "set-theme" {
                self.store.lock().await.as_mut().unwrap().set_theme(theme)?;
            } else {
                if !["light", "dark"].contains(&theme) {
                    return Err("系统主题格式不正确".into());
                }
                *self.theme.lock().unwrap() = theme.into();
            }
            let result = self.theme_state().await;
            (self.sink)("lion:theme", result.clone());
            return Ok(result);
        }
        if method == "save-profile" {
            let input = model::normalize_profile(argument(&args, 0, "设备配置")?)?;
            let supplied = args.get(1).filter(|v| !v.is_null());
            let clean = clean_secrets(supplied)?;
            let id = input["id"].as_str().unwrap().to_string();
            let lock = self
                .operations
                .lock()
                .unwrap()
                .entry(id.clone())
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone();
            let _operation = lock.lock().await;
            if self.quitting.load(Ordering::SeqCst) {
                return Err("应用正在关闭".into());
            }
            let old = self.profile(&id).await.ok();
            let changed = old.as_ref().is_some_and(|old| {
                ["host", "port", "username", "authType", "keyPath"]
                    .iter()
                    .any(|key| old[key] != input[key])
            });
            let endpoint_changed = old.as_ref().is_some_and(|old| {
                ["host", "port", "username"]
                    .iter()
                    .any(|key| old[key] != input[key])
            });
            if changed {
                self.disconnect(&id).await?;
                self.secrets.lock().unwrap().remove(&id);
            }
            let result = self
                .store
                .lock()
                .await
                .as_mut()
                .unwrap()
                .save(&input, supplied)?;
            let profile = self.profile(&id).await?;
            if supplied.is_some() {
                let mut cache = self.secrets.lock().unwrap();
                merge(cache.entry(id.clone()).or_insert(json!({})), &clean);
            }
            let device = self.devices.lock().unwrap().get(&id).cloned();
            if let Some(device) = device {
                *device.profile.lock().unwrap() = profile.clone();
                if supplied.is_some() {
                    merge(&mut device.secrets.lock().unwrap(), &clean);
                }
                let connection = device.connection.lock().unwrap().clone();
                if let Some(connection) = connection {
                    connection
                        .update_tunnels(profile["tunnels"].as_array().cloned().unwrap_or_default())
                        .await?;
                }
                let monitor = device.monitor.lock().unwrap().clone();
                if let Some(monitor) = monitor {
                    monitor.update_gpu(profile["gpu"].clone()).await?;
                }
            } else {
                self.update_disconnected_profile(&profile, endpoint_changed);
            }
            return Ok(result);
        }
        let id = string_argument(&args, 0, "设备标识")?.to_string();
        let profile = self.profile(&id).await?;
        // Read-only queries and confirmations may execute while one another is
        // awaiting a remote response; lifecycle/config changes are serialized.
        match method {
            "refresh-gpu"
            | "refresh-slurm"
            | "refresh-storage"
            | "set-slurm-scope"
            | "prepare-job-cancel"
            | "cancel-job"
            | "get-job-priority"
            | "prepare-tmp-cleanup"
            | "cleanup-tmp" => {
                let device = self.connected(&id)?;
                let monitor = device
                    .monitor
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or("监测服务尚未就绪，请稍后重试")?;
                match method {
                    "refresh-gpu" => monitor.refresh_gpu().await,
                    "refresh-slurm" => monitor.refresh_slurm().await,
                    "refresh-storage" => monitor.refresh_storage().await,
                    "set-slurm-scope" => {
                        monitor
                            .set_slurm_scope(string_argument(&args, 1, "作业范围")?)
                            .await?;
                    }
                    _ => return monitor.action(method, &args[1..]).await,
                }
                return Ok(Value::Null);
            }
            "trust-host" => {
                let fingerprint = string_argument(&args, 2, "主机指纹")?;
                let accepted = argument(&args, 1, "确认结果")?
                    .as_bool()
                    .ok_or("确认结果格式不正确")?;
                let request = self
                    .host_keys
                    .lock()
                    .unwrap()
                    .get(&id)
                    .cloned()
                    .ok_or("主机指纹请求已过期")?;
                if request["fingerprint"] != fingerprint {
                    return Err("主机指纹请求已过期".into());
                }
                let device = self
                    .devices
                    .lock()
                    .unwrap()
                    .get(&id)
                    .cloned()
                    .ok_or("连接已关闭")?;
                let sender = device
                    .trust
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or("主机指纹请求已过期")?;
                if accepted {
                    self.store
                        .lock()
                        .await
                        .as_mut()
                        .unwrap()
                        .trust(&profile, fingerprint)?;
                }
                sender
                    .send(HostDecision {
                        fingerprint: fingerprint.into(),
                        accepted,
                    })
                    .map_err(|_| "主机指纹请求已过期")?;
                self.host_keys.lock().unwrap().remove(&id);
                self.status(&id, "connecting", None);
                return Ok(Value::Null);
            }
            _ => {}
        }
        let lock = self
            .operations
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone();
        let _operation = lock.lock().await;
        if self.quitting.load(Ordering::SeqCst) {
            return Err("应用正在关闭".into());
        }
        // A queued save may have changed the endpoint while this operation
        // awaited its turn. Connect only the latest persisted profile.
        let profile = self.profile(&id).await?;
        match method {
            "connect" => {
                let existing = self.devices.lock().unwrap().get(&id).cloned();
                if existing.is_some_and(|device| device.desired.load(Ordering::SeqCst)) {
                    return Ok(Value::Null);
                }
                self.disconnect(&id).await?;
                let secrets = self.credentials(&profile, args.get(1)).await?;
                if self.quitting.load(Ordering::SeqCst) {
                    return Err("应用正在关闭".into());
                }
                let device = Arc::new(Device {
                    id: id.clone(),
                    profile: Mutex::new(profile),
                    secrets: Mutex::new(secrets),
                    desired: AtomicBool::new(true),
                    cancel: CancellationToken::new(),
                    trust: Mutex::new(None),
                    connection: Mutex::new(None),
                    monitor: Mutex::new(None),
                    task: Mutex::new(None),
                });
                self.devices
                    .lock()
                    .unwrap()
                    .insert(id.clone(), device.clone());
                self.status(&id, "connecting", None);
                let owner = self.clone();
                let worker = device.clone();
                *device.task.lock().unwrap() = Some(tokio::spawn(async move {
                    owner.run_device(worker).await;
                }));
            }
            "disconnect" => self.disconnect(&id).await?,
            "delete-profile" => {
                self.disconnect(&id).await?;
                self.store.lock().await.as_mut().unwrap().remove(&id)?;
                self.secrets.lock().unwrap().remove(&id);
                self.states.lock().unwrap().remove(&id);
            }
            "update-gpu" | "set-tunnel" => {
                let mut updated = self.profile(&id).await?;
                if method == "update-gpu" {
                    updated["gpu"] = model::gpu_settings(argument(&args, 1, "显卡设置")?)?;
                } else {
                    let tunnel_id = string_argument(&args, 1, "隧道标识")?;
                    let enabled = argument(&args, 2, "启用状态")?
                        .as_bool()
                        .ok_or("启用状态格式不正确")?;
                    let tunnel = updated["tunnels"]
                        .as_array_mut()
                        .unwrap()
                        .iter_mut()
                        .find(|t| t["id"] == tunnel_id)
                        .ok_or("隧道不存在")?;
                    tunnel["enabled"] = json!(enabled);
                }
                let result = self
                    .store
                    .lock()
                    .await
                    .as_mut()
                    .unwrap()
                    .save(&updated, None)?;
                let device = self.devices.lock().unwrap().get(&id).cloned();
                if let Some(device) = device {
                    *device.profile.lock().unwrap() = updated.clone();
                    if method == "update-gpu" {
                        let monitor = device.monitor.lock().unwrap().clone();
                        if let Some(monitor) = monitor {
                            monitor.update_gpu(updated["gpu"].clone()).await?;
                        }
                    } else {
                        let connection = device.connection.lock().unwrap().clone();
                        if let Some(connection) = connection {
                            connection
                                .set_tunnel(
                                    string_argument(&args, 1, "隧道标识")?,
                                    argument(&args, 2, "启用状态")?.as_bool().unwrap(),
                                )
                                .await?;
                        }
                    }
                } else {
                    self.update_disconnected_profile(&updated, false);
                }
                return Ok(if method == "update-gpu" {
                    result
                } else {
                    Value::Null
                });
            }
            "restart-tunnel" => {
                let tunnel_id = string_argument(&args, 1, "隧道标识")?;
                let rule = profile["tunnels"]
                    .as_array()
                    .and_then(|rules| rules.iter().find(|rule| rule["id"] == tunnel_id))
                    .ok_or("隧道不存在")?;
                if rule["enabled"] == false {
                    return Err("请先启用隧道".into());
                }
                let device = self.connected(&id)?;
                let connection = device
                    .connection
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or("请先连接设备")?;
                connection.restart_tunnel(tunnel_id).await?;
            }
            _ => return Err("不支持的应用操作".into()),
        }
        Ok(Value::Null)
    }
    fn update_disconnected_profile(&self, profile: &Value, reset: bool) {
        let id = profile["id"].as_str().unwrap();
        let mut state = model::blank_state(profile);
        if !reset {
            if let Some(previous) = self.states.lock().unwrap().get(id).cloned() {
                for key in ["events", "gpu", "slurm", "storage"] {
                    state[key] = previous[key].clone();
                }
                state["gpu"]["intervalSec"] = profile["gpu"]["intervalSec"].clone();
            }
        }
        self.states.lock().unwrap().insert(id.into(), state.clone());
        (self.sink)("lion:state", state);
    }
    pub async fn shutdown(&self) {
        self.quitting.store(true, Ordering::SeqCst);
        (self.sink)(
            "lion:shutdown",
            json!({"phase":"closing","message":"正在关闭隧道和服务器连接…"}),
        );
        let ids: Vec<_> = self.devices.lock().unwrap().keys().cloned().collect();
        for id in ids {
            let _ = self.disconnect(&id).await;
        }
        self.secrets.lock().unwrap().clear();
    }
    pub fn kill(&self) {
        for (_, device) in self.devices.lock().unwrap().drain() {
            device.desired.store(false, Ordering::SeqCst);
            device.cancel.cancel();
            if let Some(task) = device.task.lock().unwrap().take() {
                task.abort();
            }
        }
        self.secrets.lock().unwrap().clear();
    }
    pub fn quit(self: &Arc<Self>, app: &AppHandle) {
        self.quit_with_code(app, 0);
    }
    pub fn quit_with_code(self: &Arc<Self>, app: &AppHandle, code: i32) {
        if self.quitting.swap(true, Ordering::SeqCst) {
            return;
        }
        let service = self.clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(25), service.shutdown()).await;
            service.kill();
            service.finished.store(true, Ordering::SeqCst);
            app.exit(code);
        });
    }
}

pub fn apply_theme(app: &AppHandle, theme: &Value) {
    if let Some(window) = app.get_webview_window("main") {
        let preference = theme["preference"].as_str().unwrap_or("system");
        if let Some(service) = app.try_state::<Arc<Service>>() {
            let mut previous = service.theme_preference.lock().unwrap();
            if previous.as_deref() == Some(preference) {
                return;
            }
            *previous = Some(preference.into());
        }
        let native = match preference {
            "dark" => Some(tauri::Theme::Dark),
            "light" => Some(tauri::Theme::Light),
            _ => None,
        };
        let _ = window.set_theme(native);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_data_defaults_keep_existing_development_and_portable_layouts() {
        let manifest = Path::new("workspace/src-tauri");
        let executable = Path::new("installed/Lion SSH.exe");
        assert_eq!(
            select_data_directory(None, true, true, manifest, Some(executable), None).unwrap(),
            PathBuf::from("workspace/data")
        );
        assert_eq!(
            select_data_directory(None, true, false, manifest, Some(executable), None).unwrap(),
            PathBuf::from("installed/data")
        );
        assert!(
            select_data_directory(None, true, false, manifest, None, None)
                .unwrap_err()
                .contains("无法确定应用程序目录")
        );
    }

    #[test]
    fn macos_and_linux_data_use_writable_platform_directories_in_all_builds() {
        let manifest = Path::new("workspace/src-tauri");
        for base in [
            "/Users/fixture/Library/Application Support",
            "/home/fixture/.local/share",
            "/custom-xdg-data",
        ] {
            for development in [true, false] {
                assert_eq!(
                    select_data_directory(
                        None,
                        false,
                        development,
                        manifest,
                        Some(Path::new("readonly/app")),
                        Some(Path::new(base))
                    )
                    .unwrap(),
                    Path::new(base).join("tools.lion.ssh")
                );
            }
        }
        assert!(
            select_data_directory(None, false, false, manifest, None, None)
                .unwrap_err()
                .contains("LION_DATA_DIR")
        );
    }

    #[test]
    fn explicit_data_directory_takes_priority_without_changing_relative_overrides() {
        for windows in [false, true] {
            for directory in [
                PathBuf::from("custom-relative-data"),
                std::env::temp_dir().join("lion-explicit-data"),
            ] {
                assert_eq!(
                    select_data_directory(
                        Some(directory.clone()),
                        windows,
                        false,
                        Path::new(""),
                        None,
                        None
                    )
                    .unwrap(),
                    directory
                );
            }
        }
    }
    #[test]
    fn native_control_methods_are_not_exposed_to_webview() {
        for method in [
            "shutdown",
            "native-theme",
            "encrypt",
            "decrypt",
            "pick-key",
            "exec",
            "read-file",
        ] {
            assert!(!PUBLIC_METHODS.contains(&method));
        }
    }
    #[test]
    fn secrets_only_accept_bounded_strings() {
        assert!(clean_secrets(Some(&json!({"password":42}))).is_err());
        assert_eq!(
            clean_secrets(Some(
                &json!({"password":"abc","remember":true,"command":"rm"})
            ))
            .unwrap(),
            json!({"password":"abc"})
        );
    }

    fn fixture() -> (PathBuf, Arc<Service>, Value) {
        let directory = std::env::temp_dir().join(format!("lion-service-{}", uuid::Uuid::new_v4()));
        let service = Service::open(directory.clone(), "light", Arc::new(|_, _| {}));
        let profile = json!({"id":"queued-device","name":"existing","host":"127.0.0.1","port":1,"username":"fixture","authType":"password","autoReconnect":false,"tunnels":[]});
        (directory, service, profile)
    }

    #[tokio::test]
    async fn queued_connect_uses_the_profile_committed_before_its_turn() {
        let (directory, service, mut profile) = fixture();
        service
            .request("save-profile", vec![profile.clone()])
            .await
            .unwrap();
        let lock = service.operations.lock().unwrap()["queued-device"].clone();
        let guard = lock.lock().await;
        let requester = service.clone();
        let mut pending = tokio::spawn(async move {
            requester
                .request(
                    "connect",
                    vec![json!("queued-device"), json!({"password":"fixture"})],
                )
                .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut pending)
                .await
                .is_err()
        );
        // Simulate the preceding queued save committing while connect awaits.
        profile["port"] = json!(2);
        service
            .store
            .lock()
            .await
            .as_mut()
            .unwrap()
            .save(&profile, None)
            .unwrap();
        drop(guard);
        pending.await.unwrap().unwrap();
        let actual = service.devices.lock().unwrap()["queued-device"]
            .profile
            .lock()
            .unwrap()
            .clone();
        assert_eq!(actual["port"], 2);
        service.shutdown().await;
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn queued_save_is_rejected_after_shutdown() {
        let (directory, service, mut profile) = fixture();
        service
            .request("save-profile", vec![profile.clone()])
            .await
            .unwrap();
        let lock = service.operations.lock().unwrap()["queued-device"].clone();
        let guard = lock.lock().await;
        profile["name"] = json!("should not be saved");
        let requester = service.clone();
        let mut pending =
            tokio::spawn(async move { requester.request("save-profile", vec![profile]).await });
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut pending)
                .await
                .is_err()
        );
        service.shutdown().await;
        drop(guard);
        assert!(pending.await.unwrap().unwrap_err().contains("关闭"));
        let stored: Value =
            serde_json::from_slice(&std::fs::read(directory.join("profiles.json")).unwrap())
                .unwrap();
        assert_eq!(stored[0]["name"], "existing");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn invalid_credentials_cannot_remove_a_saved_secret() {
        let (directory, service, profile) = fixture();
        service
            .request("save-profile", vec![profile.clone()])
            .await
            .unwrap();
        // This validation regression test needs opaque saved data, not access
        // to an interactive macOS Keychain or Linux Secret Service in CI.
        std::fs::write(
            directory.join("credentials.json"),
            r#"{"queued-device":"native:v1:opaque-fixture"}"#,
        )
        .unwrap();
        let service = Service::open(directory.clone(), "light", Arc::new(|_, _| {}));
        let before = std::fs::read(directory.join("credentials.json")).unwrap();
        assert!(service
            .request(
                "save-profile",
                vec![profile, json!({"password":42,"remember":false})]
            )
            .await
            .is_err());
        assert_eq!(
            std::fs::read(directory.join("credentials.json")).unwrap(),
            before
        );
        service.shutdown().await;
        std::fs::remove_dir_all(directory).unwrap();
    }
}
