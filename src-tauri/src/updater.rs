//! Signed desktop updates. Windows portable builds use an isolated Rust helper
//! and a manifest-limited file transaction instead of running an installer.

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::Emitter;
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

const ENDPOINT: &str = "https://github.com/ZjDing989/LionSSH/releases/latest/download/latest.json";
const HOUR: Duration = Duration::from_secs(3600);
const MAX_ARCHIVE: usize = 256 * 1024 * 1024;
const MAX_EXTRACTED: u64 = 512 * 1024 * 1024;
const MAX_FILES: usize = 1024;
const APP_EXE: &str = "Lion SSH.exe";
const HELPER_FLAG: &str = "--apply-portable-update";
const FINISH_FLAG: &str = "--finish-portable-update";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn error_text(error: impl ToString) -> String {
    error.to_string().chars().take(500).collect()
}

fn version(value: &str) -> Result<semver::Version, String> {
    semver::Version::parse(value.trim_start_matches('v')).map_err(|error| error.to_string())
}

fn public_key() -> String {
    if let Some(value) =
        option_env!("LION_UPDATER_PUBLIC_KEY").filter(|value| !value.trim().is_empty())
    {
        return value.to_owned();
    }
    let config: Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).unwrap_or(Value::Null);
    config
        .pointer("/plugins/updater/pubkey")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn configured() -> bool {
    decode_base64(&public_key())
        .and_then(|value| {
            minisign_verify::PublicKey::decode(&value).map_err(|error| error.to_string())
        })
        .is_ok()
}

fn is_portable(executable: &Path) -> bool {
    cfg!(windows)
        && executable
            .parent()
            .is_some_and(|parent| parent.join("portable.json").is_file())
}

fn portable_target() -> String {
    format!("windows-{}-portable", std::env::consts::ARCH)
}

fn portable_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "ia32",
        other => other,
    }
}

pub struct UpdateManager {
    app: tauri::AppHandle,
    state: Mutex<Value>,
    pending: Mutex<Option<Update>>,
    operation: AsyncMutex<()>,
    busy: AtomicBool,
    started: AtomicBool,
    cancel: CancellationToken,
    portable: bool,
}

impl UpdateManager {
    pub fn new(app: tauri::AppHandle) -> Arc<Self> {
        let portable = std::env::current_exe()
            .map(|exe| is_portable(&exe))
            .unwrap_or(false);
        let manual_only = cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_none();
        Arc::new(Self {
            app,
            state: Mutex::new(
                json!({"status":"idle","currentVersion":env!("CARGO_PKG_VERSION"),
                "version":null,"notes":null,"publishedAt":null,"error":null,"downloaded":0,"total":null,
                "lastChecked":null,"nextCheckAt":null,"portable":portable,"configured":configured(),
                "manualOnly":manual_only,"releaseUrl":"https://github.com/ZjDing989/LionSSH/releases/latest"}),
            ),
            pending: Mutex::new(None),
            operation: AsyncMutex::new(()),
            busy: AtomicBool::new(false),
            started: AtomicBool::new(false),
            cancel: CancellationToken::new(),
            portable,
        })
    }

    pub fn state(&self) -> Value {
        self.state.lock().unwrap().clone()
    }

    fn patch(&self, patch: Value) {
        let mut state = self.state.lock().unwrap();
        for (key, value) in patch.as_object().unwrap() {
            state[key] = value.clone();
        }
        let snapshot = state.clone();
        drop(state);
        let _ = self.app.emit("lion:update", snapshot);
    }

    pub fn start(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let weak = Arc::downgrade(self);
        let token = self.cancel.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                let Some(manager) = weak.upgrade() else {
                    break;
                };
                if token.is_cancelled() {
                    break;
                }
                let _ = manager.check().await;
                manager.patch(json!({"nextCheckAt":now()+HOUR.as_millis() as u64}));
                drop(manager);
                tokio::select! { _ = token.cancelled()=>break, _ = tokio::time::sleep(HOUR)=>{} }
            }
        });
    }

    pub fn stop(&self) {
        self.cancel.cancel();
    }

    pub async fn check(&self) -> Result<Value, String> {
        let _operation = self.operation.lock().await;
        if self.busy.load(Ordering::Acquire) {
            return Ok(self.state());
        }
        if !configured() {
            self.patch(json!({"status":"idle","configured":false,"error":"此构建尚未配置自动更新签名公钥","lastChecked":now()}));
            return Ok(self.state());
        }
        self.patch(json!({"status":"checking","error":null}));
        let result = async {
            let mut builder = self
                .app
                .updater_builder()
                .pubkey(public_key())
                .timeout(Duration::from_secs(20))
                .endpoints(vec![ENDPOINT.parse().expect("static updater endpoint")])
                .map_err(error_text)?
                .version_comparator(|current, release| {
                    release.version > current && release.version.pre.is_empty()
                });
            if self.portable {
                builder = builder.target(portable_target());
            }
            #[cfg(windows)]
            if !self.portable {
                let executable = std::env::current_exe().map_err(error_text)?;
                let directory = executable.parent().ok_or("无法确定当前安装目录")?;
                // NSIS requires /D to be the final argument without quotes.
                // The plugin appends installer_arg values last; no shell runs.
                let mut directory_argument = std::ffi::OsString::from("/D=");
                directory_argument.push(directory.as_os_str());
                builder = builder.installer_arg(directory_argument);
            }
            let updater = builder.build().map_err(error_text)?;
            let update = tokio::select! {
                _ = self.cancel.cancelled()=>return Err("更新检查已取消".to_owned()),
                update = updater.check()=>update.map_err(error_text)?,
            };
            if let Some(update) = &update {
                validate_release_url(update.download_url.as_str())?;
            }
            Ok::<_, String>(update)
        }
        .await;
        match result {
            Ok(Some(update)) => {
                self.patch(json!({"status":"available","version":update.version,"notes":update.body,
                    "publishedAt":update.date.map(|date|date.to_string()),"lastChecked":now(),"error":null}));
                *self.pending.lock().unwrap() = Some(update);
                Ok(self.state())
            }
            Ok(None) => {
                self.pending.lock().unwrap().take();
                self.patch(json!({"status":"idle","version":null,"notes":null,"publishedAt":null,"lastChecked":now(),"error":null}));
                Ok(self.state())
            }
            Err(error) => {
                let available = self.pending.lock().unwrap().is_some();
                self.patch(json!({"status":if available{"available"}else{"error"},"error":error,"lastChecked":now()}));
                Err(error)
            }
        }
    }

    /// Download and verify completely before the caller shuts down live SSH
    /// sessions. A failed download therefore leaves the running app usable.
    pub async fn prepare(self: &Arc<Self>) -> Result<PreparedUpdate, String> {
        let _operation = self.operation.lock().await;
        if self.busy.swap(true, Ordering::AcqRel) {
            return Err("正在准备软件更新".into());
        }
        let result = self.prepare_inner().await;
        if let Err(error) = &result {
            self.busy.store(false, Ordering::Release);
            self.patch(json!({"status":"available","error":error}));
        }
        result
    }

    async fn prepare_inner(self: &Arc<Self>) -> Result<PreparedUpdate, String> {
        if cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_none() {
            return Err(
                "此 Linux 运行方式请从 GitHub Release 手动下载新版本；官方 AppImage 支持自动更新"
                    .into(),
            );
        }
        let mut update = self
            .pending
            .lock()
            .unwrap()
            .clone()
            .ok_or("当前没有可安装的新版本")?;
        if version(&update.version)? <= version(env!("CARGO_PKG_VERSION"))? {
            return Err("更新版本必须高于当前版本".into());
        }
        validate_release_url(update.download_url.as_str())?;
        update.timeout = Some(Duration::from_secs(180));
        self.patch(json!({"status":"downloading","downloaded":0,"total":null,"error":null}));
        let mut downloaded = 0u64;
        let mut last_progress = 0u64;
        let weak = Arc::downgrade(self);
        let bytes = tokio::select! {
            _ = self.cancel.cancelled()=>return Err("更新下载已取消".into()),
            result = update.download(move |count,total| {
                downloaded+=count as u64;
                if now().saturating_sub(last_progress)>100 {
                    if let Some(manager)=weak.upgrade(){manager.patch(json!({"downloaded":downloaded,"total":total}));}
                    last_progress=now();
                }
            },||{})=>result.map_err(error_text)?,
        };
        if bytes.len() > MAX_ARCHIVE {
            return Err("更新包超过大小限制".into());
        }
        update.verify(&bytes).map_err(error_text)?;
        let kind = if self.portable {
            let executable = std::env::current_exe().map_err(error_text)?;
            let signature = update.signature.clone();
            let announced = update.version.clone();
            PreparedKind::Portable(
                tokio::task::spawn_blocking(move || {
                    prepare_portable(&executable, &bytes, &signature, &announced)
                })
                .await
                .map_err(error_text)??,
            )
        } else {
            PreparedKind::Standard { update, bytes }
        };
        self.patch(json!({"status":"ready","error":null}));
        Ok(PreparedUpdate {
            owner: Arc::downgrade(self),
            kind: Some(kind),
        })
    }
}

impl Drop for UpdateManager {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub enum InstallDisposition {
    Exit,
    Restart,
}

pub struct PreparedUpdate {
    owner: Weak<UpdateManager>,
    kind: Option<PreparedKind>,
}
enum PreparedKind {
    Standard { update: Update, bytes: Vec<u8> },
    Portable(PortablePrepared),
}
struct PortablePrepared {
    temporary: tempfile::TempDir,
    job_path: PathBuf,
    helper: PathBuf,
}

impl PreparedUpdate {
    /// The caller must have completed Service::shutdown before calling this.
    pub fn install(mut self) -> Result<InstallDisposition, String> {
        if let Some(owner) = self.owner.upgrade() {
            owner.patch(json!({"status":"installing","error":null}));
        }
        let result = (|| match self.kind.take().ok_or("更新包尚未准备好")? {
            PreparedKind::Standard { update, bytes } => {
                update.install(bytes).map_err(error_text)?;
                Ok(InstallDisposition::Restart)
            }
            PreparedKind::Portable(prepared) => {
                let mut command = Command::new(&prepared.helper);
                command.arg(HELPER_FLAG).arg(&prepared.job_path);
                hide_command(&mut command);
                command.spawn().map_err(error_text)?;
                let _ = prepared.temporary.keep();
                Ok(InstallDisposition::Exit)
            }
        })();
        if let Some(owner) = self.owner.upgrade() {
            owner.busy.store(false, Ordering::Release);
            if let Err(error) = &result {
                owner.patch(json!({"status":"available","error":error}));
            }
        }
        result
    }
}

impl Drop for PreparedUpdate {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            owner.busy.store(false, Ordering::Release);
        }
    }
}

fn validate_release_url(value: &str) -> Result<(), String> {
    if !value.starts_with("https://github.com/ZjDing989/LionSSH/releases/download/") {
        return Err("更新包地址不属于 LionSSH 官方 GitHub Release".into());
    }
    Ok(())
}

fn decode_base64(value: &str) -> Result<String, String> {
    String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(value.trim())
            .map_err(error_text)?,
    )
    .map_err(error_text)
}

fn verify_portable_signature(
    bytes: &[u8],
    signature: &str,
    key: &str,
    announced: &str,
) -> Result<(), String> {
    let key = minisign_verify::PublicKey::decode(&decode_base64(key)?).map_err(error_text)?;
    let signature =
        minisign_verify::Signature::decode(&decode_base64(signature)?).map_err(error_text)?;
    key.verify(bytes, &signature, true).map_err(error_text)?;
    let signed = signature
        .trusted_comment()
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"))
        .ok_or("更新签名缺少版本信息")?;
    if version(signed)? != version(announced)? {
        return Err("更新签名版本与发布版本不一致".into());
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ManifestFile {
    path: String,
    size: u64,
    sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct PortableManifest {
    version: String,
    arch: String,
    files: Vec<ManifestFile>,
}
struct PortablePayload {
    manifest: PortableManifest,
    files: BTreeMap<String, Vec<u8>>,
}

fn safe_path(value: &str) -> Result<PathBuf, String> {
    if value.is_empty()
        || value.contains('\\')
        || value.starts_with('/')
        || value
            .chars()
            .any(|c| c < ' ' || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return Err("更新清单路径不安全".into());
    }
    let mut path = PathBuf::new();
    for component in value.split('/') {
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.ends_with('.')
            || component.ends_with(' ')
        {
            return Err("更新清单包含无效的路径段".into());
        }
        let device = component
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if ["CON", "PRN", "AUX", "NUL"].contains(&device.as_str())
            || (device.len() == 4
                && (device.starts_with("COM") || device.starts_with("LPT"))
                && matches!(device.as_bytes()[3], b'1'..=b'9'))
        {
            return Err("更新清单包含 Windows 设备路径".into());
        }
        if component.eq_ignore_ascii_case("data")
            || component.to_ascii_lowercase().starts_with(".lion-update-")
        {
            return Err("更新包不能修改用户 data 或更新工作目录".into());
        }
        path.push(component);
    }
    Ok(path)
}

fn validate_manifest(manifest: &PortableManifest) -> Result<(), String> {
    version(&manifest.version)?;
    if !matches!(
        manifest.arch.as_str(),
        "x64" | "x86_64" | "arm64" | "aarch64" | "ia32" | "i686"
    ) {
        return Err("便携包架构无效".into());
    }
    if manifest.files.is_empty() || manifest.files.len() > MAX_FILES {
        return Err("便携包文件清单数量无效".into());
    }
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    for item in &manifest.files {
        safe_path(&item.path)?;
        if item.path.eq_ignore_ascii_case("manifest.json") || !seen.insert(item.path.to_lowercase())
        {
            return Err("便携包文件清单重复或自引用".into());
        }
        if item.sha256.len() != 64 || !item.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("便携包文件校验值无效".into());
        }
        total = total.checked_add(item.size).ok_or("更新包超过大小限制")?;
        if total > MAX_EXTRACTED {
            return Err("更新包解压大小超过限制".into());
        }
    }
    if !seen.contains(&APP_EXE.to_lowercase()) || !seen.contains("portable.json") {
        return Err("便携包缺少主程序或 portable.json".into());
    }
    Ok(())
}

fn payload(bytes: &[u8], announced: &str) -> Result<PortablePayload, String> {
    if bytes.len() > MAX_ARCHIVE {
        return Err("更新包超过大小限制".into());
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(error_text)?;
    if archive.len() > MAX_FILES * 2 {
        return Err("更新 ZIP 文件数量过多".into());
    }
    let mut raw = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(error_text)?;
        let name = entry.name().trim_end_matches('/').to_owned();
        safe_path(&name)?;
        if entry.is_dir() {
            continue;
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err("更新 ZIP 不能包含符号链接".into());
        }
        if !seen.insert(name.to_lowercase()) {
            return Err("更新 ZIP 包含重复路径".into());
        }
        total = total.checked_add(entry.size()).ok_or("更新 ZIP 大小无效")?;
        if total > MAX_EXTRACTED {
            return Err("更新 ZIP 解压大小超过限制".into());
        }
        let mut data = Vec::new();
        let entry_size = entry.size();
        entry
            .by_ref()
            .take(entry_size.saturating_add(1))
            .read_to_end(&mut data)
            .map_err(error_text)?;
        if data.len() as u64 != entry_size {
            return Err("更新 ZIP 文件大小不一致".into());
        }
        raw.insert(name, data);
    }
    let manifests = raw
        .keys()
        .filter(|name| name.as_str() == "manifest.json" || name.ends_with("/manifest.json"))
        .filter(|name| {
            // Nested license inventories are not the distribution manifest.
            raw.get(*name)
                .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
                .is_some_and(|value| value.get("arch").is_some() && value.get("files").is_some())
        })
        .cloned()
        .collect::<Vec<_>>();
    if manifests.len() != 1 {
        return Err("更新 ZIP 必须包含唯一的便携分发清单".into());
    }
    let manifest_name = &manifests[0];
    let prefix = manifest_name.strip_suffix("manifest.json").unwrap();
    let manifest: PortableManifest =
        serde_json::from_slice(&raw[manifest_name]).map_err(error_text)?;
    validate_manifest(&manifest)?;
    if version(&manifest.version)? != version(announced)? {
        return Err("便携分发清单版本与更新版本不一致".into());
    }
    let expected_arch = portable_arch();
    if manifest.arch != expected_arch && manifest.arch != std::env::consts::ARCH {
        return Err("便携更新包架构与当前程序不一致".into());
    }
    let mut files = BTreeMap::new();
    for item in &manifest.files {
        let name = format!("{prefix}{}", item.path);
        let data = raw
            .remove(&name)
            .ok_or_else(|| format!("便携更新包缺少 {}", item.path))?;
        if data.len() as u64 != item.size
            || format!("{:x}", Sha256::digest(&data)) != item.sha256.to_ascii_lowercase()
        {
            return Err(format!("便携更新文件校验失败：{}", item.path));
        }
        files.insert(item.path.clone(), data);
    }
    let manifest_bytes = raw.remove(manifest_name).unwrap();
    if !raw.is_empty() {
        return Err("便携更新 ZIP 含有清单外文件".into());
    }
    let portable: Value = serde_json::from_slice(files.get("portable.json").ok_or("便携标识缺失")?)
        .map_err(error_text)?;
    if portable["portable"] != true {
        return Err("便携更新包的 portable.json 标识无效".into());
    }
    files.insert("manifest.json".into(), manifest_bytes);
    Ok(PortablePayload { manifest, files })
}

#[derive(Serialize, Deserialize)]
struct PortableJob {
    parent_pid: u32,
    destination: PathBuf,
    executable: PathBuf,
    version: String,
    signature: String,
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(error_text)?;
    }
    let mut file = File::create(path).map_err(error_text)?;
    file.write_all(bytes).map_err(error_text)?;
    file.sync_all().map_err(error_text)
}

fn prepare_portable(
    executable: &Path,
    bytes: &[u8],
    signature: &str,
    announced: &str,
) -> Result<PortablePrepared, String> {
    verify_portable_signature(bytes, signature, &public_key(), announced)?;
    let destination = executable
        .parent()
        .ok_or("无法确定便携版目录")?
        .canonicalize()
        .map_err(error_text)?;
    if executable.file_name().is_none_or(|name| name != APP_EXE) {
        return Err("请将便携版主程序文件名恢复为 Lion SSH.exe 后更新".into());
    }
    let payload = payload(bytes, announced)?;
    preflight(&destination, &payload)?;
    let temporary = tempfile::Builder::new()
        .prefix("lion-ssh-updater-")
        .tempdir()
        .map_err(error_text)?;
    let helper = temporary.path().join("helper.exe");
    fs::copy(executable, &helper).map_err(error_text)?;
    write_file(&temporary.path().join("update.zip"), bytes)?;
    let job = PortableJob {
        parent_pid: std::process::id(),
        destination,
        executable: executable.canonicalize().map_err(error_text)?,
        version: announced.to_owned(),
        signature: signature.to_owned(),
    };
    let job_path = temporary.path().join("job.json");
    write_file(&job_path, &serde_json::to_vec(&job).map_err(error_text)?)?;
    Ok(PortablePrepared {
        temporary,
        job_path,
        helper,
    })
}

fn regular_target(base: &Path, name: &str) -> Result<PathBuf, String> {
    let relative = safe_path(name)?;
    let mut path = base.to_owned();
    for part in relative.components() {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if metadata.file_attributes() & 0x400 != 0 {
                        return Err("更新路径包含重解析点".into());
                    }
                }
                if metadata.file_type().is_symlink() {
                    return Err("更新路径包含符号链接".into());
                }
                if path != base.join(&relative) && !metadata.is_dir() {
                    return Err("更新路径的父级不是目录".into());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error_text(error)),
        }
    }
    Ok(path)
}

fn preflight(destination: &Path, payload: &PortablePayload) -> Result<PortableManifest, String> {
    let old_path = regular_target(destination, "manifest.json")?;
    let old: PortableManifest = serde_json::from_slice(
        &fs::read(old_path)
            .map_err(|_| "现有便携目录缺少有效 manifest.json，请先完整解压便携版")?,
    )
    .map_err(error_text)?;
    validate_manifest(&old)?;
    let owned = old
        .files
        .iter()
        .map(|item| (item.path.to_lowercase(), item))
        .collect::<BTreeMap<_, _>>();
    for item in &old.files {
        let path = regular_target(destination, &item.path)?;
        if path.exists() {
            if !path.is_file() {
                return Err("受管程序文件被目录替代".into());
            }
            let bytes = fs::read(path).map_err(error_text)?;
            if bytes.len() as u64 != item.size
                || format!("{:x}", Sha256::digest(&bytes)) != item.sha256.to_ascii_lowercase()
            {
                return Err(format!(
                    "程序文件 {} 已被修改，请先备份后手动更新",
                    item.path
                ));
            }
        }
    }
    for item in &payload.manifest.files {
        let path = regular_target(destination, &item.path)?;
        if path.exists() && !owned.contains_key(&item.path.to_lowercase()) {
            return Err(format!("更新将覆盖用户文件 {}，请先移走该文件", item.path));
        }
        if owned
            .get(&item.path.to_lowercase())
            .is_some_and(|old| old.path != item.path)
        {
            return Err("便携文件名大小写发生变更，请手动更新该版本".into());
        }
    }
    Ok(old)
}

fn replace_file(target: &Path, staged: &Path, backup: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide = |path: &Path| {
            path.as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>()
        };
        let target = wide(target);
        let staged = wide(staged);
        let backup = wide(backup);
        let result = unsafe {
            windows_sys::Win32::Storage::FileSystem::ReplaceFileW(
                target.as_ptr(),
                staged.as_ptr(),
                backup.as_ptr(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if result == 0 {
            return Err(error_text(std::io::Error::last_os_error()));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::copy(target, backup).map_err(error_text)?;
        fs::rename(staged, target).map_err(error_text)
    }
}

fn apply_payload(
    destination: &Path,
    payload: PortablePayload,
    fail_after: Option<usize>,
) -> Result<(), String> {
    let old = preflight(destination, &payload)?;
    let workspace = tempfile::Builder::new()
        .prefix(".lion-update-")
        .tempdir_in(destination)
        .map_err(error_text)?;
    let staged = workspace.path().join("new");
    let backup = workspace.path().join("backup");
    fs::create_dir_all(&backup).map_err(error_text)?;
    for (name, bytes) in &payload.files {
        write_file(&staged.join(safe_path(name)?), bytes)?;
    }
    let mut names = old
        .files
        .iter()
        .map(|item| item.path.clone())
        .chain(payload.files.keys().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    // Commit the executable and distribution manifest last. Each existing file
    // replacement is atomic on Windows, with a rollback copy on the same disk.
    names.sort_by_key(|name| {
        (
            if name == APP_EXE {
                2
            } else if name == "manifest.json" {
                3
            } else {
                1
            },
            name.clone(),
        )
    });
    let mut changed = Vec::new();
    let result = (|| {
        for name in names {
            if fail_after.is_some_and(|count| changed.len() >= count) {
                return Err("模拟更新文件替换失败".to_owned());
            }
            let target = regular_target(destination, &name)?;
            let relative = safe_path(&name)?;
            let previous = backup.join(&relative);
            let replacement = staged.join(&relative);
            if let Some(parent) = previous.parent() {
                fs::create_dir_all(parent).map_err(error_text)?;
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(error_text)?;
            }
            let existed = target.exists();
            if replacement.exists() {
                if existed {
                    replace_file(&target, &replacement, &previous)?;
                } else {
                    fs::rename(&replacement, &target).map_err(error_text)?;
                }
            } else if existed {
                fs::rename(&target, &previous).map_err(error_text)?;
            }
            changed.push((name, existed));
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut rollback_errors = Vec::new();
        for (name, existed) in changed.into_iter().rev() {
            let target = destination.join(safe_path(&name)?);
            let previous = backup.join(safe_path(&name)?);
            let restored = if existed && previous.exists() {
                if target.exists() {
                    fs::remove_file(&target).and_then(|_| fs::rename(&previous, &target))
                } else {
                    fs::rename(&previous, &target)
                }
            } else if !existed && target.exists() {
                fs::remove_file(&target)
            } else {
                Ok(())
            };
            if let Err(failure) = restored {
                rollback_errors.push(format!("{name}: {failure}"));
            }
        }
        if !rollback_errors.is_empty() {
            let saved = workspace.keep();
            return Err(format!(
                "{error}；部分回滚未完成，备份保留在 {}：{}",
                saved.display(),
                rollback_errors.join("；")
            ));
        }
        return Err(error);
    }
    Ok(())
}

fn hide_command(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

fn wait_parent(pid: u32) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
        };
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(87) {
                return Ok(());
            }
            return Err(format!("无法等待旧程序退出：{error}"));
        }
        let result = unsafe { WaitForSingleObject(handle, 60_000) };
        unsafe { CloseHandle(handle) };
        if result != WAIT_OBJECT_0 {
            return Err("等待旧程序退出超时，更新已取消".into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        Err("便携更新助手仅支持 Windows".into())
    }
}

fn validate_job_directory(job_path: &Path) -> Result<PathBuf, String> {
    let directory = job_path
        .parent()
        .ok_or("更新任务目录无效")?
        .canonicalize()
        .map_err(error_text)?;
    let temporary = std::env::temp_dir().canonicalize().map_err(error_text)?;
    if directory.parent() != Some(temporary.as_path())
        || !directory
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("lion-ssh-updater-"))
        || job_path.file_name().is_none_or(|name| name != "job.json")
    {
        return Err("更新助手只接受自己的临时任务目录".into());
    }
    Ok(directory)
}

fn run_helper(job_path: &Path) -> Result<(), String> {
    let directory = validate_job_directory(job_path)?;
    let job: PortableJob =
        serde_json::from_slice(&fs::read(directory.join("job.json")).map_err(error_text)?)
            .map_err(error_text)?;
    let destination = job.destination.canonicalize().map_err(error_text)?;
    if job.executable.parent() != Some(destination.as_path())
        || job
            .executable
            .file_name()
            .is_none_or(|name| name != APP_EXE)
        || !destination.join("portable.json").is_file()
    {
        return Err("便携更新目标不是原程序目录".into());
    }
    let bytes = fs::read(directory.join("update.zip")).map_err(error_text)?;
    verify_portable_signature(&bytes, &job.signature, &public_key(), &job.version)?;
    if version(&job.version)? <= version(env!("CARGO_PKG_VERSION"))? {
        return Err("便携更新版本必须高于当前程序".into());
    }
    let verified = payload(&bytes, &job.version)?;
    preflight(&destination, &verified)?;
    wait_parent(job.parent_pid)?;
    let result = apply_payload(&destination, verified, None);
    if let Err(error) = &result {
        write_file(&directory.join("error.txt"), error.as_bytes())?;
    }
    let mut restart = Command::new(&job.executable);
    restart.current_dir(&destination);
    if result.is_ok() {
        restart.arg(FINISH_FLAG).arg(directory.join("job.json"));
    }
    hide_command(&mut restart);
    restart.spawn().map_err(error_text)?;
    result
}

fn finish_cleanup(job_path: &Path) -> Result<(), String> {
    let directory = validate_job_directory(job_path)?;
    let job: PortableJob =
        serde_json::from_slice(&fs::read(directory.join("job.json")).map_err(error_text)?)
            .map_err(error_text)?;
    if job.executable.canonicalize().map_err(error_text)?
        != std::env::current_exe()
            .map_err(error_text)?
            .canonicalize()
            .map_err(error_text)?
        || version(&job.version)? != version(env!("CARGO_PKG_VERSION"))?
    {
        return Err("更新清理任务与当前程序不匹配".into());
    }
    for entry in fs::read_dir(&directory).map_err(error_text)? {
        let entry = entry.map_err(error_text)?;
        let name = entry.file_name();
        if !["job.json", "update.zip", "helper.exe", "error.txt"]
            .iter()
            .any(|allowed| name == *allowed)
            || !entry.file_type().map_err(error_text)?.is_file()
        {
            return Err("更新清理目录包含额外文件，已保留".into());
        }
    }
    for _ in 0..30 {
        if fs::remove_dir_all(&directory).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("更新助手尚未退出，临时目录已保留".into())
}

/// Call before initializing Tauri or the single-instance plugin. A helper does
/// not create a window, join the app instance, or open any user data files.
pub fn run_portable_helper_from_args() -> Option<Result<(), String>> {
    let args = std::env::args_os().collect::<Vec<_>>();
    if args.get(1).is_some_and(|value| value == HELPER_FLAG) {
        return Some(match args.get(2) {
            Some(path) if args.len() == 3 => run_helper(Path::new(path)),
            _ => Err("更新助手参数无效".into()),
        });
    }
    if args.get(1).is_some_and(|value| value == FINISH_FLAG) && args.len() == 3 {
        // Windows cannot remove the helper image until its process exits.
        // Only a verified task for this exact new executable may be removed.
        let _ = finish_cleanup(Path::new(&args[2]));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_payload(version: &str, extra: &[(&str, &[u8])]) -> PortablePayload {
        let mut files = BTreeMap::from([
            (
                APP_EXE.to_owned(),
                format!("program-{version}").into_bytes(),
            ),
            ("portable.json".into(), br#"{"portable":true}"#.to_vec()),
            ("LICENSE.txt".into(), b"license".to_vec()),
        ]);
        for (name, bytes) in extra {
            files.insert((*name).to_owned(), bytes.to_vec());
        }
        let manifest = PortableManifest {
            version: version.into(),
            arch: portable_arch().into(),
            files: files
                .iter()
                .map(|(name, bytes)| ManifestFile {
                    path: name.clone(),
                    size: bytes.len() as u64,
                    sha256: format!("{:x}", Sha256::digest(bytes)),
                })
                .collect(),
        };
        files.insert(
            "manifest.json".into(),
            serde_json::to_vec(&manifest).unwrap(),
        );
        PortablePayload { manifest, files }
    }

    fn install_fixture(base: &Path, payload: &PortablePayload) {
        for (name, bytes) in &payload.files {
            write_file(&base.join(name), bytes).unwrap();
        }
    }

    #[test]
    fn rejects_traversal_data_devices_and_windows_aliases() {
        for name in [
            "../app.exe",
            "/app.exe",
            "C:/app.exe",
            "data/profiles.json",
            "Data/secret",
            "licenses/../data",
            "a\\b",
            "LICENSE.txt:stream",
            "CON",
            "licenses/nul.txt",
            "file.",
            "file ",
            ".lion-update-owned/job.json",
        ] {
            assert!(safe_path(name).is_err(), "{name}");
        }
        assert!(safe_path("licenses/LICENSE-MIT.txt").is_ok());
    }

    #[test]
    fn portable_transaction_preserves_data_and_user_files_and_prunes_owned_files() {
        let directory = tempfile::tempdir().unwrap();
        let old = test_payload("1.6.2", &[("obsolete.txt", b"old")]);
        install_fixture(directory.path(), &old);
        write_file(
            &directory.path().join("data/profiles.json"),
            b"private-data",
        )
        .unwrap();
        write_file(&directory.path().join("my-notes.txt"), b"my-notes").unwrap();
        let new = test_payload("1.7.0", &[("licenses/new.txt", b"new-notice")]);
        apply_payload(directory.path(), new, None).unwrap();
        assert_eq!(
            fs::read(directory.path().join(APP_EXE)).unwrap(),
            b"program-1.7.0"
        );
        assert_eq!(
            fs::read(directory.path().join("data/profiles.json")).unwrap(),
            b"private-data"
        );
        assert_eq!(
            fs::read(directory.path().join("my-notes.txt")).unwrap(),
            b"my-notes"
        );
        assert!(!directory.path().join("obsolete.txt").exists());
        assert!(directory.path().join("licenses/new.txt").exists());
    }

    #[test]
    fn portable_transaction_rolls_back_after_replacement_failure() {
        for fail_after in 0..=5 {
            let directory = tempfile::tempdir().unwrap();
            let old = test_payload("1.6.2", &[("obsolete.txt", b"old")]);
            install_fixture(directory.path(), &old);
            let new = test_payload("1.7.0", &[("licenses/new.txt", b"new-notice")]);
            assert!(apply_payload(directory.path(), new, Some(fail_after)).is_err());
            for (name, bytes) in old.files {
                assert_eq!(fs::read(directory.path().join(name)).unwrap(), bytes);
            }
            assert!(!directory.path().join("licenses/new.txt").exists());
        }
    }

    #[test]
    fn refuses_to_overwrite_user_edits_or_unmanaged_collision() {
        let directory = tempfile::tempdir().unwrap();
        let old = test_payload("1.6.2", &[]);
        install_fixture(directory.path(), &old);
        write_file(&directory.path().join("LICENSE.txt"), b"user-edit").unwrap();
        assert!(preflight(directory.path(), &test_payload("1.7.0", &[]))
            .unwrap_err()
            .contains("已被修改"));
        install_fixture(directory.path(), &old);
        write_file(&directory.path().join("notes.txt"), b"user-note").unwrap();
        assert!(preflight(
            directory.path(),
            &test_payload("1.7.0", &[("notes.txt", b"new")])
        )
        .unwrap_err()
        .contains("用户文件"));
    }

    #[test]
    fn validates_release_source_and_semver() {
        assert!(validate_release_url(
            "https://github.com/ZjDing989/LionSSH/releases/download/v1.7.0/update.zip"
        )
        .is_ok());
        assert!(validate_release_url(
            "https://github.com.example.com/ZjDing989/LionSSH/releases/download/x"
        )
        .is_err());
        assert_eq!(version("v1.7.0").unwrap(), version("1.7.0").unwrap());
    }

    fn zipped(payload: PortablePayload, extra: Option<(&str, &[u8])>) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, bytes) in payload.files {
            writer
                .start_file(format!("release-portable/{name}"), options)
                .unwrap();
            writer.write_all(&bytes).unwrap();
        }
        if let Some((name, bytes)) = extra {
            writer.start_file(name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn zip_validates_manifest_hash_version_and_rejects_hidden_user_data() {
        let archive = zipped(test_payload("1.7.0", &[]), None);
        let parsed = payload(&archive, "1.7.0").unwrap();
        assert_eq!(parsed.files[APP_EXE], b"program-1.7.0");
        assert!(payload(&archive, "1.8.0").is_err());
        let data = zipped(
            test_payload("1.7.0", &[]),
            Some(("release-portable/data/profiles.json", b"private")),
        );
        assert!(payload(&data, "1.7.0").is_err());
        let undeclared = zipped(
            test_payload("1.7.0", &[]),
            Some(("release-portable/undeclared.exe", b"unmanaged")),
        );
        assert!(payload(&undeclared, "1.7.0").is_err());
        let mut changed = test_payload("1.7.0", &[]);
        changed
            .files
            .insert("LICENSE.txt".into(), b"tampered".to_vec());
        assert!(payload(&zipped(changed, None), "1.7.0").is_err());
    }

    #[test]
    fn signatures_reject_modified_payload_metadata_and_version_replay() {
        // Public, throwaway test vector from tauri-plugin-updater's offline tests.
        let key="untrusted comment: minisign public key: A49EF4746FA6E6D7\nRWTX5qZvdPSepDGWg2bz8n6ArQnBMwgQK1geVosorcbLzcunD+KJ+cYe";
        let signature="untrusted comment: signature from tauri secret key\nRUTX5qZvdPSepPF2zsjvP/L/TQtBLRnWRvaq+4omK1UEugtQp/USrGiI1v762sDy3id3aor7bRC9ETyeot6lMHHrWoZ5hjUM1Qc=\ntrusted comment: timestamp:1791649051\tfile:payload\tversion:2.0.0\nDDq2CLOEl5cQmC2eJRHlnZPVViOZsu1CjpPKQoei4A/XBfkDqGO0B+QMrcrHgEQ5qV3mne75tdFWhJpqUXh9AA==";
        let encoded = base64::engine::general_purpose::STANDARD.encode(signature);
        let key = base64::engine::general_purpose::STANDARD.encode(key);
        assert!(verify_portable_signature(b"test", &encoded, &key, "v2.0.0").is_ok());
        assert!(verify_portable_signature(b"tampered", &encoded, &key, "2.0.0").is_err());
        assert!(verify_portable_signature(b"test", &encoded, &key, "9.0.0").is_err());
        let altered = base64::engine::general_purpose::STANDARD
            .encode(signature.replace("version:2.0.0", "version:9.0.0"));
        assert!(verify_portable_signature(b"test", &altered, &key, "9.0.0").is_err());
    }
}
