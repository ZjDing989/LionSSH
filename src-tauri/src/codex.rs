//! Codex installation discovery and explicitly requested updates over SSH.
//! Only fixed operations are exposed. Paths come from a bounded probe and are
//! quoted as shell literals; the renderer cannot supply a command or package.
use crate::ssh::{CommandOutput, Executor};
use async_trait::async_trait;
use semver::Version;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

type Events = Arc<dyn Fn(&str, Value) + Send + Sync>;
const MAX_OUTPUT: usize = 262_144;
const REGISTRY_URL: &str = "https://registry.npmjs.org/@openai/codex/latest";
const INSTALLER_URL: &str = "https://chatgpt.com/codex/install.sh";

pub fn blank_state() -> Value {
    json!({"status":"unknown","installed":null,"currentVersion":null,"latestVersion":null,
        "updateAvailable":false,"installMethod":null,"executablePath":null,"canInstall":false,
        "unsupportedReason":null,"lastChecked":null,"lastResult":null,"error":null,
        "restartStatus":null,"restartMessage":null,"log":[]})
}
fn now() -> u64 {
    crate::service::now()
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
fn command(script: &str) -> String {
    format!("sh -lc {}", quote(script))
}
fn version(value: &str) -> Result<Version, String> {
    let value = value
        .trim()
        .strip_prefix("codex-cli ")
        .unwrap_or(value.trim());
    let value = value.strip_prefix('v').unwrap_or(value);
    Version::parse(value).map_err(|_| "Codex 返回了无法识别的版本号".into())
}
fn clipped(value: &str, count: usize) -> String {
    value
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .take(count)
        .collect()
}
fn failure(output: &CommandOutput, label: &str) -> String {
    let detail = if output.stderr.trim().is_empty() {
        &output.stdout
    } else {
        &output.stderr
    };
    format!("{label}：{}", clipped(detail.trim(), 1600))
}

// Values are hex encoded so paths, spaces, shell startup banners and version
// output cannot break the record framing. No remote text is evaluated locally.
const PROBE: &str = r#"# Lion Codex probe V1
set -eu
emit() { printf '%s\t' "$1"; printf '%s' "$2" | od -An -tx1 | tr -d ' \n'; printf '\n'; }
resolve() {
  r=$1; n=0
  while [ -L "$r" ]; do
    n=$((n+1)); [ "$n" -le 24 ] || return 1
    l=$(readlink "$r") || return 1
    case "$l" in /*) r=$l;; *) r=$(dirname "$r")/$l;; esac
  done
  d=$(CDPATH= cd -P "$(dirname "$r")" 2>/dev/null && pwd) || return 1
  printf '%s/%s\n' "$d" "$(basename "$r")"
}
writable_parent() {
  w=$1
  while [ ! -e "$w" ]; do p=$(dirname "$w"); [ "$p" != "$w" ] || return 1; w=$p; done
  [ -d "$w" ] && [ -w "$w" ]
}
printf 'LION_CODEX_PROBE_V1\n'
os=$(uname -s); arch=$(uname -m)
emit os "$os"; emit arch "$arch"
native_home=${CODEX_HOME:-$HOME/.codex}
bin_dir=${CODEX_INSTALL_DIR:-$HOME/.local/bin}
path=$(command -v codex 2>/dev/null || true)
case "$path" in /*) ;; *) path='';; esac
if [ -z "$path" ]; then
  for candidate in "$bin_dir/codex" "$HOME/.npm-global/bin/codex" "$HOME/.npm/bin/codex"; do
    if [ -x "$candidate" ]; then path=$candidate; break; fi
  done
fi
if [ -z "$path" ]; then
  found=''
  for candidate in "$HOME"/.nvm/versions/node/*/bin/codex; do
    [ -x "$candidate" ] || continue
    if [ -n "$found" ]; then emit ambiguous 1; emit installed 1; exit 0; fi
    found=$candidate
  done
  path=$found
fi
method=unknown; real=''; current=''; npm=''; npm_root=''; npm_prefix=''; allowed=0; reason=''
if [ -n "$path" ]; then
  emit installed 1
  real=$(resolve "$path" || true)
  bin_dir=$(dirname "$path")
  PATH="$bin_dir:$PATH"; export PATH
  current=$("$path" --version 2>/dev/null || true)
  case "$real" in
    */packages/standalone/releases/*/bin/codex|*/packages/standalone/releases/*/codex)
      possible_home=${real%%/packages/standalone/releases/*}
      selected=$(resolve "$possible_home/packages/standalone/current/bin/codex" 2>/dev/null || true)
      [ -x "$selected" ] || selected=$(resolve "$possible_home/packages/standalone/current/codex" 2>/dev/null || true)
      if [ "$selected" = "$real" ]; then method=native; native_home=$possible_home; fi
      ;;
  esac
  if [ "$method" = unknown ]; then
    npm=$bin_dir/npm; [ -x "$npm" ] || npm=$(command -v npm 2>/dev/null || true)
    case "$npm" in /*) ;; *) npm='';; esac
    if [ -n "$npm" ]; then
      npm_root=$("$npm" root -g 2>/dev/null || true)
      npm_root=$(CDPATH= cd -P "$npm_root" 2>/dev/null && pwd) || npm_root=''
      npm_prefix=$("$npm" prefix -g 2>/dev/null || true)
      if [ -n "$npm_root" ]; then
        package=$(CDPATH= cd -P "$npm_root/@openai/codex" 2>/dev/null && pwd) || package=''
        case "$real" in "$package/bin/codex.js"|"$package/bin/codex")
          if [ -n "$package" ] && command -v node >/dev/null 2>&1 && node -e 'try{process.exit(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).name==="@openai/codex"?0:1)}catch{process.exit(1)}' "$package/package.json"; then method=npm; fi;;
        esac
      fi
    fi
  fi
else emit installed 0; method=native
fi
case "$os:$arch" in Linux:x86_64|Linux:amd64|Linux:aarch64|Linux:arm64|Darwin:x86_64|Darwin:arm64|Darwin:aarch64) ;; *) reason='该服务器系统或处理器暂不支持自动安装';; esac
if [ -z "$reason" ]; then
  if [ "$method" = native ]; then
    if ! command -v curl >/dev/null 2>&1 && ! command -v wget >/dev/null 2>&1; then reason='服务器需要 curl 或 wget 才能安装 Codex'
    elif ! command -v tar >/dev/null 2>&1 || ! command -v mktemp >/dev/null 2>&1; then reason='服务器缺少 tar 或 mktemp'
    elif ! writable_parent "$native_home/packages/standalone" || ! writable_parent "$bin_dir"; then reason='当前 SSH 用户没有 Codex 安装目录的写入权限'
    else allowed=1; fi
  elif [ "$method" = npm ]; then
    if ! writable_parent "$npm_root/@openai" || ! writable_parent "$bin_dir"; then reason='当前 SSH 用户没有 npm 全局安装目录的写入权限；请在服务器中调整权限后重试'
    else allowed=1; fi
  else reason='无法确认当前 Codex 属于官方脚本安装还是当前 npm 全局安装，已避免覆盖'; fi
fi
emit path "$path"; emit real "$real"; emit current "$current"; emit method "$method"
emit native_home "$native_home"; emit bin_dir "$bin_dir"; emit npm "$npm"; emit npm_root "$npm_root"; emit npm_prefix "$npm_prefix"
emit allowed "$allowed"; emit reason "$reason"
"#;

fn records(output: &str, header: &str) -> Result<HashMap<String, String>, String> {
    let (_, output) = output
        .split_once(&format!("{header}\n"))
        .ok_or("服务器未返回有效的 Codex 检测结果（需要 POSIX shell）")?;
    let mut result = HashMap::new();
    for line in output.lines() {
        let (key, encoded) = line.split_once('\t').ok_or("Codex 检测结果格式不正确")?;
        if key.is_empty()
            || key.len() > 32
            || !key.bytes().all(|c| c.is_ascii_lowercase() || c == b'_')
            || encoded.len() > 16_384
            || encoded.len() % 2 != 0
        {
            return Err("Codex 检测结果超出限制".into());
        }
        let mut bytes = Vec::with_capacity(encoded.len() / 2);
        for pair in encoded.as_bytes().chunks_exact(2) {
            let pair = std::str::from_utf8(pair).map_err(|_| "Codex 检测结果格式不正确")?;
            bytes.push(u8::from_str_radix(pair, 16).map_err(|_| "Codex 检测结果格式不正确")?);
        }
        let value = String::from_utf8(bytes).map_err(|_| "Codex 路径不是有效的 UTF-8")?;
        if value.chars().any(|c| c.is_control()) || result.insert(key.into(), value).is_some() {
            return Err("Codex 检测结果包含无效字段".into());
        }
    }
    Ok(result)
}

#[derive(Debug, Clone)]
struct Probe {
    installed: bool,
    current: Option<Version>,
    path: String,
    real: String,
    method: String,
    native_home: String,
    bin_dir: String,
    npm: String,
    npm_root: String,
    npm_prefix: String,
    allowed: bool,
    reason: Option<String>,
    os: String,
}
impl Probe {
    fn parse(output: &str) -> Result<Self, String> {
        let map = records(output, "LION_CODEX_PROBE_V1")?;
        if map.get("ambiguous").is_some_and(|v| v == "1") {
            return Err(
                "发现多个未加入登录 PATH 的 Codex 安装，请先在服务器中选择要使用的 Node/Codex 版本"
                    .into(),
            );
        }
        let get = |key: &str| map.get(key).cloned().unwrap_or_default();
        let installed = match get("installed").as_str() {
            "1" => true,
            "0" => false,
            _ => return Err("Codex 安装检测结果不完整".into()),
        };
        let method = get("method");
        if !["npm", "native", "unknown"].contains(&method.as_str()) {
            return Err("Codex 安装方式检测结果不正确".into());
        }
        let current = if installed {
            version(&get("current")).ok()
        } else {
            None
        };
        let mut reason = (!get("reason").is_empty()).then(|| get("reason"));
        let mut allowed = get("allowed") == "1";
        for key in ["native_home", "bin_dir"] {
            if !get(key).starts_with('/') {
                return Err("Codex 安装目录不是有效的绝对路径".into());
            }
        }
        if installed && (!get("path").starts_with('/') || !get("real").starts_with('/')) {
            return Err("Codex 可执行文件路径无效".into());
        }
        if method == "npm"
            && ["npm", "npm_root", "npm_prefix"]
                .iter()
                .any(|key| !get(key).starts_with('/'))
        {
            return Err("npm 安装目录检测不完整".into());
        }
        if installed && current.is_none() {
            allowed = false;
            reason =
                Some("已发现 Codex，但无法读取其版本，请先在服务器中修复 Codex 运行环境".into());
        }
        Ok(Self {
            installed,
            current,
            path: get("path"),
            real: get("real"),
            method,
            native_home: get("native_home"),
            bin_dir: get("bin_dir"),
            npm: get("npm"),
            npm_root: get("npm_root"),
            npm_prefix: get("npm_prefix"),
            allowed,
            reason,
            os: get("os"),
        })
    }
    fn state(&self, latest: Option<&Version>) -> Value {
        json!({"installed":self.installed,"currentVersion":self.current.as_ref().map(ToString::to_string),
            "latestVersion":latest.map(ToString::to_string),"updateAvailable":self.current.as_ref().zip(latest).is_some_and(|(current,latest)|latest>current),
            "installMethod":self.method,"executablePath":if self.installed {Some(self.path.as_str())}else{None},
            "canInstall":self.allowed,"unsupportedReason":self.reason,"lastChecked":now()})
    }
}

#[async_trait]
trait VersionSource: Send + Sync {
    async fn latest(&self) -> Result<Version, String>;
}
struct Registry;
impl Registry {
    fn client() -> Result<reqwest::Client, String> {
        // reqwest's rustls-no-provider feature requires explicit setup. This
        // backend also runs independently of Tauri's updater (including the
        // native test harness), so it cannot rely on that plugin doing it.
        // Keep an existing application provider; concurrent installs are safe.
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("LionSSH/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| format!("无法初始化版本检查：{e}"))
    }
}
#[async_trait]
impl VersionSource for Registry {
    async fn latest(&self) -> Result<Version, String> {
        let client = Self::client()?;
        let mut response = client
            .get(REGISTRY_URL)
            .send()
            .await
            .map_err(|e| format!("无法检查 Codex 最新版本：{e}"))?
            .error_for_status()
            .map_err(|e| format!("Codex 版本服务暂时不可用：{e}"))?;
        if response.content_length().is_some_and(|n| n > 65_536) {
            return Err("Codex 版本信息过大".into());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("读取 Codex 版本信息失败：{e}"))?
        {
            if body.len() + chunk.len() > 65_536 {
                return Err("Codex 版本信息过大".into());
            }
            body.extend_from_slice(&chunk);
        }
        let result: Value =
            serde_json::from_slice(&body).map_err(|_| "Codex 版本服务返回了无效数据")?;
        if result["name"] != "@openai/codex" {
            return Err("Codex 版本服务返回了错误的软件包".into());
        }
        let latest = version(result["version"].as_str().ok_or("Codex 最新版本缺失")?)?;
        if !latest.pre.is_empty() || !latest.build.is_empty() {
            return Err("Codex 最新稳定版信息不正确".into());
        }
        Ok(latest)
    }
}

#[derive(Clone, Debug)]
struct RestartPlan {
    unit: String,
    pid: u32,
    started: u64,
}
#[derive(Debug)]
struct ManagedDaemon {
    version: Version,
    path: String,
}
fn daemon_script(probe: &Probe, operation: &str) -> String {
    // The operation is selected by Rust, never supplied through IPC.
    debug_assert!(["version", "update"].contains(&operation));
    format!("# Lion Codex daemon {operation} V1\nset -eu\nPATH={}:\"$PATH\"; export PATH\nCODEX_HOME={} {} app-server daemon {operation}\n",quote(&probe.bin_dir),quote(&probe.native_home),quote(&probe.path))
}
fn daemon_version(output: &str, probe: &Probe) -> Result<ManagedDaemon, String> {
    let begin = output.find('{').ok_or("Codex daemon 未返回版本信息")?;
    let result: Value = serde_json::from_str(output[begin..].trim())
        .map_err(|_| "Codex daemon 版本信息格式不正确")?;
    if result["status"] != "running" || result["backend"] != "pid" {
        return Err("没有确认到运行中的 Codex 自管 daemon".into());
    }
    let path = result["managedCodexPath"]
        .as_str()
        .ok_or("Codex daemon 管理路径缺失")?;
    let home = probe.native_home.trim_end_matches('/');
    if path.chars().any(char::is_control)
        || path.split('/').any(|part| part == "..")
        || !["app-server-daemon", "standalone"]
            .iter()
            .any(|kind| path.starts_with(&format!("{home}/packages/{kind}/")))
    {
        return Err("Codex daemon 未归属于当前用户的 Codex 管理目录".into());
    }
    let current = version(
        result["appServerVersion"]
            .as_str()
            .ok_or("Codex daemon 运行版本缺失")?,
    )?;
    Ok(ManagedDaemon {
        version: current,
        path: path.into(),
    })
}
fn daemon_update_status(output: &str) -> Result<(), String> {
    let begin = output.find('{').ok_or("Codex daemon 未返回更新结果")?;
    let result: Value = serde_json::from_str(output[begin..].trim())
        .map_err(|_| "Codex daemon 更新结果格式不正确")?;
    match result["status"].as_str() {
        Some("updated" | "noUpdate") => Ok(()),
        Some("unsupported") => {
            Err("该服务器上的 Codex daemon 暂不支持官方自动更新，请在服务器中手动处理".into())
        }
        _ => Err("Codex daemon 返回了未知的更新状态".into()),
    }
}
// A terminal cannot be reattached by an SSH command. Only a currently running,
// same-user systemd Codex app-server is eligible, with executable/PID/starttime
// checked both before installation and immediately before restarting it.
fn runtime_script(probe: &Probe) -> String {
    format!(
        r#"# Lion Codex runtime V1
set -eu
printf 'LION_CODEX_RUNTIME_V1\n'
uid=$(id -u); running=unknown
if command -v pgrep >/dev/null 2>&1; then if pgrep -u "$uid" -x codex >/dev/null 2>&1; then running=yes; else code=$?; if [ "$code" = 1 ]; then running=no; fi; fi; fi
if [ {method} = npm ] && [ "$running" = no ]; then running=unknown; fi
printf 'RUNNING\t%s\n' "$running"
if [ "$(uname -s)" != Linux ] || ! command -v systemctl >/dev/null 2>&1; then exit 0; fi
units=$(systemctl --user list-units --type=service --state=running --no-legend --plain --no-pager 2>/dev/null | awk 'NR<=128 {{print $1}}') || exit 0
for unit in $units; do
  case "$unit" in *codex*.service) ;; *) continue;; esac
  case "$unit" in *[!a-zA-Z0-9_.@:-]*) continue;; esac
  pid=$(systemctl --user show "$unit" -p MainPID --value 2>/dev/null || true)
  case "$pid" in ''|0|*[!0-9]*) continue;; esac
  actual_uid=$(awk '/^Uid:/ {{print $2 ":" $3}}' "/proc/$pid/status" 2>/dev/null || true)
  [ "$actual_uid" = "$uid:$uid" ] || continue
  exe=$(readlink "/proc/$pid/exe" 2>/dev/null || true)
  [ "$exe" = {real} ] || continue
  subcommand=$(tr '\000' '\n' <"/proc/$pid/cmdline" 2>/dev/null | sed -n '2p')
  [ "$subcommand" = app-server ] || continue
  start=$(systemctl --user show "$unit" -p ExecStart --value 2>/dev/null || true)
  direct=0
  for executable in {visible} {real}; do
    expected="{{ path=$executable ; argv[]=$executable app-server"
    case "$start" in "$expected ;"*|"$expected "*) direct=1;; esac
  done
  [ "$direct" = 1 ] || continue
  started=$(sed 's/^.*) //' "/proc/$pid/stat" 2>/dev/null | awk '{{print $20}}')
  case "$started" in ''|*[!0-9]*) continue;; esac
  printf 'SERVICE\t%s\t%s\t%s\n' "$unit" "$pid" "$started"
done
"#,
        real = quote(&probe.real),
        visible = quote(&probe.path),
        method = quote(&probe.method)
    )
}
fn runtime(output: &str) -> Result<(Option<RestartPlan>, &'static str), String> {
    let (_, text) = output
        .split_once("LION_CODEX_RUNTIME_V1\n")
        .ok_or("Codex 运行状态检测结果无效")?;
    let mut plans = Vec::new();
    let mut running = "unknown";
    for line in text.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        match fields.as_slice() {
            ["RUNNING", value] if ["yes", "no", "unknown"].contains(value) => {
                running = match *value {
                    "yes" => "yes",
                    "no" => "no",
                    _ => "unknown",
                }
            }
            ["SERVICE", unit, pid, started]
                if unit.len() <= 200
                    && unit.ends_with(".service")
                    && unit.contains("codex")
                    && unit
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_.@:-".contains(&c)) =>
            {
                let pid = pid.parse::<u32>().map_err(|_| "Codex 服务 PID 无效")?;
                let started = started
                    .parse::<u64>()
                    .map_err(|_| "Codex 服务启动时间无效")?;
                if pid == 0 || started == 0 {
                    return Err("Codex 服务标识无效".into());
                }
                plans.push(RestartPlan {
                    unit: (*unit).into(),
                    pid,
                    started,
                });
            }
            _ => return Err("Codex 运行状态检测结果格式不正确".into()),
        }
    }
    Ok((
        if plans.len() == 1 { plans.pop() } else { None },
        if plans.len() > 1 { "unknown" } else { running },
    ))
}
fn install_script(probe: &Probe, latest: &Version) -> Result<String, String> {
    if !probe.allowed {
        return Err(probe
            .reason
            .clone()
            .unwrap_or("当前 Codex 安装方式不支持自动更新".into()));
    }
    let prefix = format!("PATH={}:\"$PATH\"; export PATH\n", quote(&probe.bin_dir));
    if probe.method == "npm" {
        // Verify the same global root again, and use the npm executable paired
        // with the selected installation, preserving NVM/custom prefixes.
        Ok(format!("# Lion Codex npm install V1\nset -eu\n{prefix}root=$({npm} root -g)\nroot=$(CDPATH= cd -P \"$root\" && pwd)\n[ \"$root\" = {root} ] || {{ echo 'npm 安装目录已变化，请重新检查' >&2; exit 1; }}\n{npm} install --global --prefix {npm_prefix} --no-audit --no-fund -- @openai/codex@{latest}\n",npm=quote(&probe.npm),root=quote(&probe.npm_root),npm_prefix=quote(&probe.npm_prefix)))
    } else if probe.method == "native" {
        Ok(format!(
            r#"# Lion Codex native install V1
set -eu
{prefix}installer=$(mktemp)
trap 'rm -f "$installer"' EXIT HUP INT TERM
if command -v curl >/dev/null 2>&1; then curl -fsSL --proto '=https' --proto-redir '=https' --connect-timeout 15 --max-time 90 {url} -o "$installer"
else wget -q -T 90 -O "$installer" {url}; fi
env CODEX_NON_INTERACTIVE=1 CODEX_RELEASE=latest CODEX_INSTALL_DAEMON_ONLY=0 CODEX_INSTALL_DEFER_SELECTION=0 CODEX_INSTALL_IF_LATEST=0 CODEX_INSTALL_IF_CURRENT=0 CODEX_INSTALLER_USE_RELEASES_OPENAI_COM=1 CODEX_INSTALL_DIR={bin} CODEX_HOME={home} sh "$installer"
"#,
            url = quote(INSTALLER_URL),
            bin = quote(&probe.bin_dir),
            home = quote(&probe.native_home)
        ))
    } else {
        Err("无法确认 Codex 安装方式，已避免覆盖".into())
    }
}
fn restart_script(plan: &RestartPlan, before: &Probe, probe: &Probe) -> String {
    format!(
        r#"# Lion Codex service restart V1
set -eu
unit={unit}; pid={pid}; started={started}; uid=$(id -u)
[ "$(systemctl --user show "$unit" -p MainPID --value)" = "$pid" ] || {{ echo '服务进程已变化，请手动重启' >&2; exit 1; }}
[ "$(awk '/^Uid:/ {{print $2 ":" $3}}' "/proc/$pid/status")" = "$uid:$uid" ] || exit 1
[ "$(sed 's/^.*) //' "/proc/$pid/stat" | awk '{{print $20}}')" = "$started" ] || exit 1
old_exe=$(readlink "/proc/$pid/exe")
[ "$old_exe" = {old_real} ] || [ "$old_exe" = {deleted_real} ] || exit 1
[ "$(tr '\000' '\n' <"/proc/$pid/cmdline" | sed -n '2p')" = app-server ] || exit 1
start=$(systemctl --user show "$unit" -p ExecStart --value)
direct=0
for executable in {visible} {old_real}; do
  expected="{{ path=$executable ; argv[]=$executable app-server"
  case "$start" in "$expected ;"*|"$expected "*) direct=1;; esac
done
[ "$direct" = 1 ] || {{ echo '服务启动命令已变化，请手动重启' >&2; exit 1; }}
systemctl --user restart "$unit"
systemctl --user is-active --quiet "$unit"
new_pid=$(systemctl --user show "$unit" -p MainPID --value)
case "$new_pid" in ''|0|*[!0-9]*) echo '服务未返回有效的新进程' >&2; exit 1;; esac
[ "$(awk '/^Uid:/ {{print $2 ":" $3}}' "/proc/$new_pid/status")" = "$uid:$uid" ] || exit 1
[ "$(readlink "/proc/$new_pid/exe")" = {real} ] || {{ echo '服务未使用本次更新的 Codex，请手动检查服务配置' >&2; exit 1; }}
[ "$(tr '\000' '\n' <"/proc/$new_pid/cmdline" | sed -n '2p')" = app-server ] || exit 1
printf 'LION_CODEX_RESTARTED_V1\n'
"#,
        unit = quote(&plan.unit),
        pid = plan.pid,
        started = plan.started,
        real = quote(&probe.real),
        old_real = quote(&before.real),
        visible = quote(&before.path),
        deleted_real = quote(&format!("{} (deleted)", before.real))
    )
}

pub struct CodexTool {
    executor: Arc<dyn Executor>,
    versions: Arc<dyn VersionSource>,
    events: Events,
    state: Mutex<Value>,
    operation: AsyncMutex<()>,
    cancel: CancellationToken,
}
impl CodexTool {
    pub fn new(executor: Arc<dyn Executor>, events: Events, previous: Value) -> Arc<Self> {
        let result = Self::with_versions(executor, events, Arc::new(Registry));
        if previous.is_object() {
            *result.state.lock().unwrap() = previous;
        }
        result
    }
    fn with_versions(
        executor: Arc<dyn Executor>,
        events: Events,
        versions: Arc<dyn VersionSource>,
    ) -> Arc<Self> {
        Arc::new(Self {
            executor,
            versions,
            events,
            state: Mutex::new(blank_state()),
            operation: AsyncMutex::new(()),
            cancel: CancellationToken::new(),
        })
    }
    pub fn stop(&self) {
        self.cancel.cancel();
    }
    fn update(&self, change: Value) -> Value {
        let value = {
            let mut state = self.state.lock().unwrap();
            state
                .as_object_mut()
                .unwrap()
                .extend(change.as_object().unwrap().clone());
            state.clone()
        };
        if !self.cancel.is_cancelled() {
            (self.events)("codex", value.clone());
        }
        value
    }
    fn log(&self, message: &str) {
        let mut log = self.state.lock().unwrap()["log"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for line in message.lines().map(str::trim).filter(|s| !s.is_empty()) {
            log.push(json!(clipped(line, 500)));
        }
        if log.len() > 60 {
            log.drain(..log.len() - 60);
        }
        self.update(json!({"log":log}));
    }
    async fn exec(&self, script: &str, seconds: u64) -> Result<CommandOutput, String> {
        let command = command(script);
        tokio::select! { biased; _=self.cancel.cancelled()=>Err("SSH 连接已断开，Codex 操作已取消".into()), result=self.executor.exec(&command,Duration::from_secs(seconds),MAX_OUTPUT)=>result }
    }
    async fn probe(&self) -> Result<Probe, String> {
        let output = self.exec(PROBE, 35).await?;
        if output.exit_code != Some(0) {
            return Err(failure(&output, "检测服务器 Codex 失败"));
        }
        Probe::parse(&output.stdout)
    }
    async fn latest(&self) -> Result<Version, String> {
        tokio::select! { biased; _=self.cancel.cancelled()=>Err("SSH 连接已断开，Codex 操作已取消".into()), result=self.versions.latest()=>result }
    }
    async fn inspect(&self) -> Result<(Probe, Version), String> {
        let (probe, latest) = tokio::join!(self.probe(), self.latest());
        let probe = probe?;
        self.update(probe.state(latest.as_ref().ok()));
        Ok((probe, latest?))
    }
    fn fail(&self, error: String) -> Result<Value, String> {
        self.update(json!({"status":"error","error":clipped(&error,2000)}));
        Err(error)
    }
    pub async fn refresh(&self) -> Result<Value, String> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| "Codex 操作正在进行，请稍后重试")?;
        self.update(
            json!({"status":"checking","error":null,"latestVersion":null,"updateAvailable":false}),
        );
        match self.inspect().await {
            Ok(_) => Ok(self.update(json!({"status":"ready","error":null}))),
            Err(error) => self.fail(error),
        }
    }
    pub async fn install(&self) -> Result<Value, String> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| "Codex 操作正在进行，请稍后重试")?;
        self.update(json!({"status":"checking","error":null,"lastResult":null,"restartStatus":null,"restartMessage":null,"log":[]}));
        match self.install_inner().await {
            Ok(result) => Ok(result),
            Err(error) => self.fail(error),
        }
    }
    async fn install_inner(&self) -> Result<Value, String> {
        let (before, latest) = self.inspect().await?;
        if before
            .current
            .as_ref()
            .is_some_and(|current| current >= &latest)
        {
            return Ok(self.update(
                json!({"status":"ready","lastResult":"当前 Codex 已是最新版本，无需更新"}),
            ));
        }
        let script = install_script(&before, &latest)?;
        let daemon = if before.installed {
            match self.exec(&daemon_script(&before, "version"), 20).await {
                Ok(output) if output.exit_code == Some(0) => {
                    daemon_version(&output.stdout, &before).ok()
                }
                _ => None,
            }
        } else {
            None
        };
        let (plan, running) = if daemon.is_none() && before.os == "Linux" {
            match self.exec(&runtime_script(&before), 30).await {
                Ok(output) if output.exit_code == Some(0) => {
                    runtime(&output.stdout).unwrap_or((None, "unknown"))
                }
                _ => (None, "unknown"),
            }
        } else {
            (None, "unknown")
        };
        self.update(json!({"status":"installing"}));
        self.log(if before.installed {
            "正在按已识别的安装方式更新 Codex…"
        } else {
            "正在使用官方安装脚本安装 Codex…"
        });
        let output = self.exec(&script, 600).await?;
        self.log(&output.stdout);
        if !output.stderr.trim().is_empty() {
            self.log(&output.stderr);
        }
        if output.exit_code != Some(0) {
            if let Ok(partial) = self.probe().await {
                self.update(partial.state(Some(&latest)));
            }
            return Err(failure(
                &output,
                if before.installed {
                    "Codex 更新失败（未使用 sudo）"
                } else {
                    "Codex 安装失败（未使用 sudo）"
                },
            ));
        }
        let after = self.probe().await?;
        self.update(after.state(Some(&latest)));
        let actual = after
            .current
            .as_ref()
            .ok_or("安装结束后无法读取 Codex 版本，请检查服务器环境")?;
        if actual < &latest
            || before
                .current
                .as_ref()
                .is_some_and(|current| actual < current)
        {
            return Err("安装后的 Codex 版本未达到最新版本，请检查服务器网络或安装目录".into());
        }
        if after.method != before.method || (before.installed && after.path != before.path) {
            return Err("安装后的 Codex 路径或管理方式发生变化，请在服务器中检查 PATH".into());
        }
        // The native release can advance while the npm metadata request is in
        // flight. Report the verified installed version rather than a downgrade.
        self.update(json!({"latestVersion":actual.to_string(),"updateAvailable":false}));
        let (restart_status, restart_message) = if let Some(daemon) = daemon {
            self.update(json!({"status":"restarting"}));
            self.log(&format!(
                "正在更新并重启运行中的 Codex daemon {}…",
                daemon.version
            ));
            // Reconfirm after the CLI download: a user may have stopped or
            // replaced their daemon while installation was in progress.
            let recheck = self.exec(&daemon_script(&after, "version"), 20).await;
            let same = recheck
                .as_ref()
                .ok()
                .filter(|o| o.exit_code == Some(0))
                .and_then(|o| daemon_version(&o.stdout, &after).ok())
                .is_some_and(|current| current.path == daemon.path);
            if !same {
                (
                    "manual",
                    "Codex CLI 已更新；原 daemon 已停止或管理路径已变化，请在服务器中确认后重启"
                        .into(),
                )
            } else {
                match self.exec(&daemon_script(&after, "update"), 600).await {
                    Ok(output) if output.exit_code == Some(0) => {
                        self.log(&output.stdout);
                        if let Err(error) = daemon_update_status(&output.stdout) {
                            ("failed", format!("Codex CLI 已更新；{error}"))
                        } else {
                            match self.exec(&daemon_script(&after,"version"),30).await {
                            Ok(checked) if checked.exit_code==Some(0)=>match daemon_version(&checked.stdout,&after) {
                                Ok(current) if current.version>=*actual && current.version>daemon.version=>("restarted",format!("Codex CLI 已更新，运行中的 daemon 已更新至 {} 并确认可用",current.version)),
                                Ok(current) if current.version>=*actual && current.version==daemon.version=>("already-current",format!("Codex CLI 已更新；daemon 已是 {} 最新版并保持运行",current.version)),
                                _=>("failed","Codex CLI 已更新，但 daemon 的运行版本未达到最新版本，请检查服务器中的 daemon 更新状态".into())
                            },
                            _=>("failed","Codex CLI 已更新，但 daemon 更新后未确认恢复运行，请在服务器中检查".into())
                        }
                        }
                    }
                    Ok(output) => (
                        "failed",
                        format!(
                            "Codex CLI 已更新，但 daemon 更新失败：{}",
                            failure(&output, "官方 daemon 更新命令失败")
                        ),
                    ),
                    Err(error) => (
                        "failed",
                        format!("Codex CLI 已更新，但 daemon 更新失败：{error}"),
                    ),
                }
            }
        } else if let Some(plan) = plan {
            self.update(json!({"status":"restarting"}));
            self.log(&format!("正在重启已确认的用户服务 {}…", plan.unit));
            match self.exec(&restart_script(&plan, &before, &after), 45).await {
                Ok(output)
                    if output.exit_code == Some(0)
                        && output
                            .stdout
                            .lines()
                            .any(|l| l == "LION_CODEX_RESTARTED_V1") =>
                {
                    ("restarted", format!("已更新并重启用户服务 {}", plan.unit))
                }
                Ok(output) => (
                    "failed",
                    format!(
                        "Codex 已更新，但用户服务重启未验证成功：{}",
                        failure(&output, "请检查服务")
                    ),
                ),
                Err(error) => ("failed", format!("Codex 已更新，服务重启失败：{error}")),
            }
        } else if running == "no" {
            (
                "not-running",
                "未发现当前用户运行中的 Codex；下次启动将使用新版本".into(),
            )
        } else {
            ("manual","Codex 已更新。交互式 Codex 请在原终端退出后重新打开；未确认可安全自动重启的用户服务".into())
        };
        self.log(&restart_message);
        Ok(self.update(json!({"status":"ready","error":null,"lastResult":format!("Codex {}成功：{}",if before.installed {"更新"}else{"安装"},actual),"restartStatus":restart_status,"restartMessage":restart_message})))
    }
}

#[cfg(test)]
#[path = "codex_shell_tests.rs"]
mod shell_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    #[test]
    fn registry_client_initializes_tls_without_tauri_or_any_network_request() {
        assert!(Registry::client().is_ok());
        // Also exercise the already-initialized path used by subsequent
        // refreshes or an application whose updater installed the provider.
        assert!(Registry::client().is_ok());
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }
    fn hex(value: &str) -> String {
        value
            .as_bytes()
            .iter()
            .map(|n| format!("{n:02x}"))
            .collect()
    }
    fn probe_output(
        installed: bool,
        method: &str,
        current: &str,
        allowed: bool,
        reason: &str,
    ) -> String {
        let path = if method == "npm" {
            "/home/fixture/node/bin/codex"
        } else {
            "/home/fixture/.local/bin/codex"
        };
        let real = if method == "npm" {
            "/home/fixture/node/lib/node_modules/@openai/codex/bin/codex.js".into()
        } else {
            format!("/home/fixture/.codex/packages/standalone/releases/{current}-x86_64-unknown-linux-musl/bin/codex")
        };
        let values = [
            ("installed", if installed { "1" } else { "0" }),
            ("method", method),
            ("current", if installed { current } else { "" }),
            ("path", if installed { path } else { "" }),
            ("real", if installed { &real } else { "" }),
            ("native_home", "/home/fixture/.codex"),
            (
                "bin_dir",
                if method == "npm" {
                    "/home/fixture/node/bin"
                } else {
                    "/home/fixture/.local/bin"
                },
            ),
            ("npm", "/home/fixture/node/bin/npm"),
            ("npm_root", "/home/fixture/node/lib/node_modules"),
            ("npm_prefix", "/home/fixture/node"),
            ("allowed", if allowed { "1" } else { "0" }),
            ("reason", reason),
            ("os", "Linux"),
            ("arch", "x86_64"),
        ];
        format!(
            "login banner\nLION_CODEX_PROBE_V1\n{}",
            values
                .iter()
                .map(|(key, value)| format!("{key}\t{}\n", hex(value)))
                .collect::<String>()
        )
    }
    struct Latest {
        failed: AtomicBool,
    }
    #[async_trait]
    impl VersionSource for Latest {
        async fn latest(&self) -> Result<Version, String> {
            if self.failed.load(Ordering::SeqCst) {
                Err("fixture registry unavailable".into())
            } else {
                Ok(Version::new(0, 3, 0))
            }
        }
    }
    struct Fake {
        method: String,
        installed: AtomicBool,
        current: Mutex<String>,
        allowed: AtomicBool,
        fail_install: AtomicBool,
        block: AtomicBool,
        entered: tokio::sync::Notify,
        daemon: AtomicUsize,
        daemon_update: Mutex<String>,
        runtime: Mutex<String>,
        commands: Mutex<Vec<String>>,
    }
    impl Fake {
        fn new(method: &str, installed: bool) -> Arc<Self> {
            Arc::new(Self {
                method: method.into(),
                installed: AtomicBool::new(installed),
                current: Mutex::new("0.2.0".into()),
                allowed: AtomicBool::new(true),
                fail_install: AtomicBool::new(false),
                block: AtomicBool::new(false),
                entered: tokio::sync::Notify::new(),
                daemon: AtomicUsize::new(0),
                daemon_update: Mutex::new("updated".into()),
                runtime: Mutex::new("LION_CODEX_RUNTIME_V1\nRUNNING\tyes\n".into()),
                commands: Mutex::new(Vec::new()),
            })
        }
    }
    #[async_trait]
    impl Executor for Fake {
        async fn exec(
            &self,
            command: &str,
            _timeout: Duration,
            _max: usize,
        ) -> Result<CommandOutput, String> {
            self.commands.lock().unwrap().push(command.into());
            if self.block.load(Ordering::SeqCst) {
                self.entered.notify_one();
                std::future::pending::<()>().await;
            }
            let mut code = 0;
            let stdout = if command.contains("Lion Codex probe V1") {
                probe_output(
                    self.installed.load(Ordering::SeqCst),
                    &self.method,
                    &self.current.lock().unwrap(),
                    self.allowed.load(Ordering::SeqCst),
                    if self.allowed.load(Ordering::SeqCst) {
                        ""
                    } else {
                        "当前 SSH 用户没有安装目录的写入权限"
                    },
                )
            } else if command.contains("Lion Codex daemon version V1") {
                let mode = self.daemon.load(Ordering::SeqCst);
                if mode == 0 {
                    code = 1;
                    String::new()
                } else {
                    json!({"status":"running","backend":"pid","managedCodexPath":"/home/fixture/.codex/packages/app-server-daemon/current/bin/codex","appServerVersion":if mode==1 {"0.2.0"}else{"0.3.0"}}).to_string()
                }
            } else if command.contains("Lion Codex daemon update V1") {
                let status = self.daemon_update.lock().unwrap().clone();
                if ["updated", "noUpdate"].contains(&status.as_str()) {
                    self.daemon.store(2, Ordering::SeqCst);
                }
                json!({"status":status,"installedVersion":"0.3.0","runningVersion":"0.3.0"})
                    .to_string()
            } else if command.contains("Lion Codex runtime V1") {
                self.runtime.lock().unwrap().clone()
            } else if command.contains("Lion Codex service restart V1") {
                "LION_CODEX_RESTARTED_V1\n".into()
            } else if command.contains("Lion Codex npm install V1")
                || command.contains("Lion Codex native install V1")
            {
                if self.fail_install.load(Ordering::SeqCst) {
                    code = 1;
                    String::new()
                } else {
                    self.installed.store(true, Ordering::SeqCst);
                    *self.current.lock().unwrap() = "0.3.0".into();
                    "installed successfully\n".into()
                }
            } else {
                return Err(format!("Unexpected fixed operation: {command}"));
            };
            Ok(CommandOutput {
                stdout,
                stderr: if code == 0 {
                    String::new()
                } else {
                    "EACCES fixture permission denied".into()
                },
                exit_code: Some(code),
            })
        }
    }
    fn harness(fake: Arc<Fake>) -> (Arc<CodexTool>, Arc<Latest>, Arc<Mutex<Vec<Value>>>) {
        let latest = Arc::new(Latest {
            failed: AtomicBool::new(false),
        });
        let states = Arc::new(Mutex::new(Vec::new()));
        let sent = states.clone();
        let tool = CodexTool::with_versions(
            fake,
            Arc::new(move |event, value| {
                assert_eq!(event, "codex");
                sent.lock().unwrap().push(value);
            }),
            latest.clone(),
        );
        (tool, latest, states)
    }
    #[tokio::test]
    async fn discovers_npm_and_native_and_preserves_their_update_method() {
        for method in ["npm", "native"] {
            let fake = Fake::new(method, true);
            let (tool, _, states) = harness(fake.clone());
            let checked = tool.refresh().await.unwrap();
            assert_eq!(checked["installMethod"], method);
            assert_eq!(checked["currentVersion"], "0.2.0");
            assert_eq!(checked["latestVersion"], "0.3.0");
            assert_eq!(checked["updateAvailable"], true);
            let result = tool.install().await.unwrap();
            assert_eq!(result["currentVersion"], "0.3.0");
            assert_eq!(result["updateAvailable"], false);
            assert_eq!(result["restartStatus"], "manual");
            let commands = fake.commands.lock().unwrap();
            assert!(commands
                .iter()
                .any(|c| c.contains(&format!("Lion Codex {method} install V1"))));
            assert!(!commands
                .iter()
                .any(|c| c.contains("pkill") || c.contains("killall") || c.contains("sudo")));
            assert!(states
                .lock()
                .unwrap()
                .iter()
                .any(|s| s["status"] == "installing"));
            assert_eq!(states.lock().unwrap().last().unwrap()["status"], "ready");
        }
    }
    #[tokio::test]
    async fn installs_missing_codex_with_official_native_script_without_starting_new_service() {
        let fake = Fake::new("native", false);
        *fake.runtime.lock().unwrap() = "LION_CODEX_RUNTIME_V1\nRUNNING\tno\n".into();
        let (tool, _, _) = harness(fake.clone());
        let checked = tool.refresh().await.unwrap();
        assert_eq!(checked["installed"], false);
        assert_eq!(checked["currentVersion"], Value::Null);
        assert_eq!(checked["canInstall"], true);
        let result = tool.install().await.unwrap();
        assert_eq!(result["installed"], true);
        assert_eq!(result["restartStatus"], "not-running");
        let commands = fake.commands.lock().unwrap();
        let install = commands
            .iter()
            .find(|c| c.contains("Lion Codex native install V1"))
            .unwrap();
        assert!(install.contains(INSTALLER_URL));
        assert!(install.contains("CODEX_RELEASE=latest"));
        assert!(install.contains("CODEX_NON_INTERACTIVE=1"));
        assert!(!commands
            .iter()
            .any(|c| c.contains("daemon update") || c.contains("service restart")));
    }
    #[tokio::test]
    async fn registry_failure_does_not_misreport_installed_codex_as_absent_or_run_an_installer() {
        let fake = Fake::new("npm", true);
        let (tool, latest, _) = harness(fake.clone());
        latest.failed.store(true, Ordering::SeqCst);
        assert!(tool.refresh().await.unwrap_err().contains("registry"));
        let state = tool.state.lock().unwrap().clone();
        assert_eq!(state["installed"], true);
        assert_eq!(state["currentVersion"], "0.2.0");
        assert_eq!(state["latestVersion"], Value::Null);
        assert_eq!(state["status"], "error");
        assert!(tool.install().await.is_err());
        assert!(!fake
            .commands
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains(" install V1")));
    }
    #[tokio::test]
    async fn permission_and_installer_failures_remain_retryable_without_sudo() {
        let fake = Fake::new("npm", true);
        let (tool, _, _) = harness(fake.clone());
        fake.allowed.store(false, Ordering::SeqCst);
        assert!(tool.install().await.unwrap_err().contains("写入权限"));
        assert!(!fake
            .commands
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains(" install V1")));
        fake.allowed.store(true, Ordering::SeqCst);
        fake.fail_install.store(true, Ordering::SeqCst);
        assert!(tool
            .install()
            .await
            .unwrap_err()
            .contains("permission denied"));
        assert_eq!(tool.state.lock().unwrap()["currentVersion"], "0.2.0");
        assert_eq!(tool.state.lock().unwrap()["updateAvailable"], true);
        fake.fail_install.store(false, Ordering::SeqCst);
        assert!(tool.install().await.is_ok());
    }
    #[tokio::test]
    async fn will_not_downgrade_or_replace_unknown_installations() {
        let fake = Fake::new("native", true);
        *fake.current.lock().unwrap() = "0.4.0-beta.1".into();
        let (tool, _, _) = harness(fake.clone());
        assert_eq!(tool.refresh().await.unwrap()["updateAvailable"], false);
        assert!(tool.install().await.is_ok());
        assert!(!fake
            .commands
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains(" install V1")));
        let fake = Fake::new("unknown", true);
        fake.allowed.store(false, Ordering::SeqCst);
        let (tool, _, _) = harness(fake.clone());
        assert!(tool.install().await.is_err());
        assert!(!fake
            .commands
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains(" install V1")));
    }
    #[tokio::test]
    async fn only_updates_already_running_managed_daemon_then_verifies_running_version() {
        let fake = Fake::new("npm", true);
        fake.daemon.store(1, Ordering::SeqCst);
        let (tool, _, states) = harness(fake.clone());
        let result = tool.install().await.unwrap();
        assert_eq!(result["restartStatus"], "restarted");
        assert!(result["restartMessage"].as_str().unwrap().contains("0.3.0"));
        let commands = fake.commands.lock().unwrap();
        assert!(commands
            .iter()
            .any(|c| c.contains("Lion Codex daemon update V1")));
        assert!(!commands
            .iter()
            .any(|c| c.contains("--from-cli") || c.contains("--yes") || c.contains("runtime V1")));
        assert!(states
            .lock()
            .unwrap()
            .iter()
            .any(|s| s["status"] == "restarting"));
    }
    #[tokio::test]
    async fn daemon_noop_is_reported_truthfully_and_unsupported_is_not_treated_as_success() {
        for (mode, status, expected) in [
            (2, "noUpdate", "already-current"),
            (1, "unsupported", "failed"),
            (1, "unexpected", "failed"),
        ] {
            let fake = Fake::new("npm", true);
            fake.daemon.store(mode, Ordering::SeqCst);
            *fake.daemon_update.lock().unwrap() = status.into();
            let (tool, _, _) = harness(fake);
            let result = tool.install().await.unwrap();
            assert_eq!(result["restartStatus"], expected);
            assert_eq!(result["currentVersion"], "0.3.0");
            if expected == "already-current" {
                assert!(result["restartMessage"]
                    .as_str()
                    .unwrap()
                    .contains("保持运行"));
            }
        }
    }
    #[tokio::test]
    async fn a_unique_verified_systemd_service_is_restarted_but_ambiguous_services_are_not() {
        for multiple in [false, true] {
            let fake = Fake::new("native", true);
            *fake.runtime.lock().unwrap() = format!(
                "LION_CODEX_RUNTIME_V1\nRUNNING\tyes\nSERVICE\tcodex.service\t123\t456\n{}",
                if multiple {
                    "SERVICE\tcodex-other.service\t124\t457\n"
                } else {
                    ""
                }
            );
            let (tool, _, _) = harness(fake.clone());
            let result = tool.install().await.unwrap();
            assert_eq!(
                result["restartStatus"],
                if multiple { "manual" } else { "restarted" }
            );
            let commands = fake.commands.lock().unwrap();
            let restarts: Vec<_> = commands
                .iter()
                .filter(|c| c.contains("Lion Codex service restart V1"))
                .collect();
            assert_eq!(restarts.len(), if multiple { 0 } else { 1 });
            if let Some(restart) = restarts.first() {
                assert!(restart.contains("MainPID"));
                assert!(restart.contains("/proc/$pid/stat"));
                assert!(restart.contains("Uid:"));
                assert!(restart.contains("ExecStart"));
                assert!(restart.contains("app-server"));
            }
        }
    }
    #[tokio::test]
    async fn cancelled_connection_stops_pending_operation_and_suppresses_all_late_events() {
        let fake = Fake::new("native", true);
        fake.block.store(true, Ordering::SeqCst);
        let (tool, _, states) = harness(fake.clone());
        let running = tool.clone();
        let task = tokio::spawn(async move { running.refresh().await });
        fake.entered.notified().await;
        assert!(tool.install().await.unwrap_err().contains("正在进行"));
        tool.stop();
        let count = states.lock().unwrap().len();
        assert!(tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        assert_eq!(states.lock().unwrap().len(), count);
    }
    #[test]
    fn bounded_protocol_rejects_control_characters_invalid_versions_and_field_spoofing() {
        let good = probe_output(true, "npm", "0.2.0", true, "");
        assert!(Probe::parse(&good).is_ok());
        assert!(Probe::parse(&format!("{good}path\t{}\n", hex("/evil"))).is_err());
        assert!(Probe::parse(&good.replace(
            &hex("/home/fixture/node/bin/codex"),
            &hex("/home/fixture\n/codex")
        ))
        .is_err());
        let unknown = Probe::parse(&probe_output(true, "npm", "broken", true, "")).unwrap();
        assert!(!unknown.allowed);
        assert!(unknown.reason.unwrap().contains("无法读取"));
        assert!(records(
            "LION_CODEX_PROBE_V1\ninstalled\tzz\n",
            "LION_CODEX_PROBE_V1"
        )
        .is_err());
        let probe = Probe::parse(&good).unwrap();
        assert!(daemon_version(&json!({"status":"running","backend":"pid","managedCodexPath":"/other-user/.codex/packages/standalone/current/bin/codex","appServerVersion":"0.2.0"}).to_string(),&probe).is_err());
        assert!(daemon_version(&json!({"status":"running","managedCodexPath":"/home/fixture/.codex/packages/standalone/current/bin/codex","appServerVersion":"0.2.0"}).to_string(),&probe).is_err());
    }
    #[test]
    fn shell_paths_are_literals_and_only_fixed_operations_can_be_constructed() {
        let mut probe = Probe::parse(&probe_output(true, "npm", "0.2.0", true, "")).unwrap();
        probe.npm = "/home/a 'b/$(touch injected)/npm".into();
        let script = install_script(&probe, &Version::new(0, 3, 0)).unwrap();
        assert!(script.contains("'/home/a '\"'\"'b/$(touch injected)/npm'"));
        assert!(script.contains("@openai/codex@0.3.0"));
        assert!(PROBE.contains("readFileSync(process.argv[1]"));
        assert!(PROBE.contains("@openai/codex"));
    }
}
