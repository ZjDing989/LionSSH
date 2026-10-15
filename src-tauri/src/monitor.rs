//! Per-connection monitoring and narrowly scoped remote operations.
//!
//! Renderer input can select an operation, a validated job ID, or a one-use
//! confirmation token. It can never supply a remote command or shell fragment.
use crate::ssh::Executor;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

const GPU_QUERY: &str = "nvidia-smi --query-gpu=index,uuid,name,memory.total,memory.used,utilization.gpu --format=csv,noheader,nounits";
const COMPUTE_QUERY: &str = "nvidia-smi --query-compute-apps=gpu_uuid,pid,process_name,used_memory --format=csv,noheader,nounits";
const SUMMARY_QUERY: &str = "nvidia-smi";
const MAX_OUTPUT: usize = 4_194_304;
const SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const TOKEN_LIFETIME: u64 = 60_000;
type Events = Arc<dyn Fn(&str, Value) + Send + Sync>;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn re(pattern: &str) -> Regex {
    // A large Slurm queue can contain tens of thousands of rows. Compile each
    // fixed parser expression once; Regex clones share the compiled program.
    static EXPRESSIONS: std::sync::OnceLock<std::sync::RwLock<HashMap<String, Regex>>> =
        std::sync::OnceLock::new();
    let expressions = EXPRESSIONS.get_or_init(|| std::sync::RwLock::new(HashMap::new()));
    if let Some(expression) = expressions
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(pattern)
    {
        return expression.clone();
    }
    let expression = Regex::new(pattern).expect("constant regular expression");
    expressions
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(pattern.to_owned(), expression.clone());
    expression
}
fn text(value: &Value, field: &str) -> String {
    value[field].as_str().unwrap_or("").to_owned()
}
fn limited(value: &str) -> String {
    value.chars().take(500).collect()
}
fn merge(target: &mut Value, source: Value) {
    if let (Some(t), Some(s)) = (target.as_object_mut(), source.as_object()) {
        t.extend(s.clone());
    }
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub fn normalize_gpu(options: &Value) -> Value {
    let number = |key: &str, default: f64, min: f64, max: f64| {
        options[key]
            .as_f64()
            .filter(|n| n.is_finite())
            .unwrap_or(default)
            .clamp(min, max)
    };
    json!({"enabled": options["enabled"] != false, "intervalSec": number("intervalSec",5.,2.,300.).round() as u64,
        "idleUtil":number("idleUtil",5.,0.,100.), "idleMemoryMiB":number("idleMemoryMiB",512.,0.,1_000_000.),
        "stableSamples":number("stableSamples",2.,1.,10.).round() as u64})
}

fn strip_ansi(input: &str) -> String {
    re(r"\x1b\[[0-9;]*m").replace_all(input, "").into_owned()
}
fn gpu_availability(error: &QueryError) -> &'static str {
    let output = strip_ansi(&error.output);
    if re(r"(?i)failed to initialize NVML|couldn['’]?t communicate with the NVIDIA driver|driver(?:/library)? version mismatch|insufficient permissions?|permission denied|access (?:is )?denied|timed? out|timeout|查询超时").is_match(&output){return if error.detected{"available"}else{"unknown"};}
    if re(r"(?i)\bNo devices were found\b|\bNo NVIDIA GPUs? (?:were |was )?found\b")
        .is_match(&output)
    {
        return "absent";
    }
    if re(r#"(?i)(?:nvidia-smi(?:\.exe)?(?:['"’])?\s*:?\s*(?:command )?not found|(?:command )?not found\s*:?\s*nvidia-smi|nvidia-smi[^\r\n]*is not recognized|无法找到[^\r\n]*nvidia-smi|nvidia-smi[^\r\n]*不是内部或外部命令)"#).is_match(&output){return "unavailable";}
    if error.detected {
        "available"
    } else {
        "unknown"
    }
}
fn gpu_reason(error: &QueryError, availability: &str) -> String {
    match availability {
        "absent" => "未检测到 NVIDIA 显卡".into(),
        "unavailable" => "未安装或无法找到 nvidia-smi，无法判断是否存在 NVIDIA 显卡".into(),
        _ => limited(&error.message),
    }
}
fn parse_csv(input: &str) -> Result<Vec<Vec<String>>, String> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = input.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if quoted && chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = !quoted;
                }
            }
            ',' if !quoted => {
                row.push(field.trim().to_owned());
                field.clear();
            }
            '\r' | '\n' if !quoted => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(field.trim().to_owned());
                field.clear();
                if row.iter().any(|v| !v.is_empty()) {
                    rows.push(std::mem::take(&mut row));
                } else {
                    row.clear();
                }
            }
            _ => field.push(c),
        }
    }
    if quoted {
        return Err("nvidia-smi 返回了不完整的 CSV 数据".into());
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field.trim().to_owned());
        if row.iter().any(|v| !v.is_empty()) {
            rows.push(row);
        }
    }
    Ok(rows)
}
fn measurement(value: &str) -> Value {
    if value.is_empty() || re(r"(?i)N/A|Not Supported|Unknown").is_match(value) {
        return Value::Null;
    }
    value
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.)
        .map(|v| json!(v))
        .unwrap_or(Value::Null)
}
fn parse_gpu_rows(input: &str) -> Result<Vec<Value>, String> {
    let mut cards = Vec::new();
    for row in parse_csv(input)? {
        if row.len() != 6 || !re(r"^\d+$").is_match(&row[0]) || !row[1].starts_with("GPU-") {
            return Err("无法解析 nvidia-smi 显卡数据".into());
        }
        let index = row[0]
            .parse::<u32>()
            .map_err(|_| "无法解析 nvidia-smi 显卡编号")?;
        cards.push(json!({"index":index,"uuid":row[1],"name":row[2],"memoryTotalMiB":measurement(&row[3]),"memoryUsedMiB":measurement(&row[4]),"utilization":measurement(&row[5])}));
    }
    if cards.is_empty() {
        return Err("未检测到 NVIDIA 显卡".into());
    }
    cards.sort_by_key(|v| v["index"].as_u64());
    Ok(cards)
}
fn parse_compute_rows(input: &str) -> Result<Vec<Value>, String> {
    if input.trim().is_empty() || re(r"(?i)^No running processes found\.?$").is_match(input.trim())
    {
        return Ok(Vec::new());
    }
    parse_csv(input)?
        .into_iter()
        .map(|row| {
            if row.len() != 4
                || !re(r"^(GPU-|MIG-)").is_match(&row[0])
                || !re(r"^\d+$").is_match(&row[1])
            {
                return Err("无法解析 nvidia-smi 计算进程数据".into());
            }
            let pid = row[1]
                .parse::<u64>()
                .map_err(|_| "无法解析 nvidia-smi 计算进程编号")?;
            Ok(json!({"uuid":row[0],"pid":pid,"name":row[2],"memoryUsedMiB":measurement(&row[3])}))
        })
        .collect()
}
fn parse_summary(input: &str) -> Result<(Vec<Value>, Vec<Value>, bool), String> {
    let input = strip_ansi(input);
    if !re(r"(?i)NVIDIA-SMI").is_match(&input) {
        return Err("无法识别 nvidia-smi 状态表".into());
    }
    let lines = input.lines().collect::<Vec<_>>();
    let process_index = lines
        .iter()
        .position(|v| re(r"(?i)^\s*\|\s*Processes\s*:").is_match(v));
    let mut rows: Vec<Value> = Vec::new();
    let mut measured = Vec::new();
    let heading = re(r"(?i)^(\d+)\s+(.+?)\s+(?:On|Off|N/A)$");
    let memory = re(r"(?i)^(\d+(?:\.\d+)?|N/A)\s*(?:MiB)?\s*/\s*(\d+(?:\.\d+)?|N/A)\s*(?:MiB)?$");
    for line in &lines[..process_index.unwrap_or(lines.len())] {
        if !line.trim_start().starts_with('|') {
            continue;
        }
        let cells = line.split('|').skip(1).collect::<Vec<_>>();
        if cells.len() < 4 {
            continue;
        }
        let cells = cells[..cells.len() - 1]
            .iter()
            .map(|v| v.trim())
            .collect::<Vec<_>>();
        if let Some(h) = heading.captures(cells[0]) {
            if measured.last() == Some(&false) {
                return Err("nvidia-smi 显卡状态表不完整".into());
            }
            let index = h[1]
                .parse::<u32>()
                .map_err(|_| "无法解析 nvidia-smi 显卡编号")?;
            rows.push(json!({"index":index,"uuid":format!("GPU-summary-{index}"),"name":h[2].trim(),"memoryTotalMiB":null,"memoryUsedMiB":null,"utilization":null}));
            measured.push(false);
            continue;
        }
        if re(r"^\d+\s+").is_match(cells[0]) {
            return Err("无法解析 nvidia-smi 显卡名称行".into());
        }
        if rows.is_empty() || measured.last() == Some(&true) {
            continue;
        }
        if let Some(m) = memory.captures(cells[1]) {
            let row = rows.last_mut().unwrap();
            row["memoryUsedMiB"] = measurement(&m[1]);
            row["memoryTotalMiB"] = measurement(&m[2]);
            row["utilization"] = re(r"^(\d+(?:\.\d+)?)%\s")
                .captures(cells[2])
                .map(|u| measurement(&u[1]))
                .unwrap_or(Value::Null);
            *measured.last_mut().unwrap() = true;
        }
    }
    let indices = rows
        .iter()
        .map(|v| v["index"].as_u64().unwrap())
        .collect::<HashSet<_>>();
    if rows.is_empty() || measured.contains(&false) || indices.len() != rows.len() {
        return Err("nvidia-smi 显卡状态表不完整".into());
    }
    let mut processes = Vec::new();
    let mut complete = false;
    if let Some(index) = process_index {
        let mut started = false;
        let mut closed = false;
        let mut recognized = 0;
        let mut empty = false;
        let mut invalid = false;
        let mut header = false;
        let process = re(
            r"(?i)^(\d+)\s+(?:(?:N/A|\d+)\s+(?:N/A|\d+)\s+)?(\d+)\s+(C\+G|C|M|G)\s+(.+?)\s+(\d+(?:\.\d+)?|N/A)\s*(?:MiB)?$",
        );
        for line in &lines[index + 1..] {
            if re(r"(?i)GPU\s+(?:GI\s+CI\s+)?PID\s+Type\s+Process name").is_match(line) {
                header = true;
            }
            if re(r"^\s*\|=+\|\s*$").is_match(line) {
                started = true;
                continue;
            }
            if !started {
                continue;
            }
            if re(r"^\s*\+-+\+\s*$").is_match(line) {
                closed = true;
                break;
            }
            if !line.trim_start().starts_with('|') {
                if !line.trim().is_empty() && !line.trim().starts_with("[LION read-only snapshot") {
                    invalid = true;
                }
                continue;
            }
            let content = line
                .trim()
                .trim_start_matches('|')
                .trim_end_matches('|')
                .trim();
            if content.is_empty() {
                continue;
            }
            if re(r"(?i)^No running processes found\.?$").is_match(content) {
                empty = true;
                continue;
            }
            if let Some(p) = process.captures(content) {
                let index = p[1].parse::<u64>().unwrap_or(u64::MAX);
                if !indices.contains(&index) {
                    invalid = true;
                    continue;
                }
                recognized += 1;
                if !p[3].eq_ignore_ascii_case("G") {
                    processes.push(json!({"uuid":format!("GPU-summary-{}",&p[1]),"pid":p[2].parse::<u64>().map_err(|_|"进程编号无效")?,"name":&p[4],"memoryUsedMiB":measurement(&p[5])}));
                }
            } else {
                invalid = true;
            }
        }
        complete = header && closed && !invalid && (recognized > 0 || empty);
    }
    for row in &mut rows {
        row["processesKnown"] = json!(complete);
    }
    Ok((rows, processes, complete))
}
fn summarize_gpu(
    rows: &[Value],
    processes: &[Value],
    options: &Value,
    previous: &[Value],
) -> Value {
    let settings = normalize_gpu(options);
    let known = rows
        .iter()
        .map(|v| v["uuid"].as_str().unwrap_or(""))
        .collect::<HashSet<_>>();
    let unknown_processes = processes
        .iter()
        .any(|v| !known.contains(v["uuid"].as_str().unwrap_or("")));
    let cards=rows.iter().map(|card|{
        let own=processes.iter().filter(|p|p["uuid"]==card["uuid"]).cloned().collect::<Vec<_>>();
        let candidate=!unknown_processes && card["processesKnown"]!=false && own.is_empty()
            && card["utilization"].as_f64().is_some_and(|v|v<=settings["idleUtil"].as_f64().unwrap())
            && card["memoryUsedMiB"].as_f64().is_some_and(|v|v<=settings["idleMemoryMiB"].as_f64().unwrap());
        let stable=if candidate{previous.iter().find(|v|v["uuid"]==card["uuid"]).and_then(|v|v["stableCount"].as_u64()).unwrap_or(0).saturating_add(1).min(settings["stableSamples"].as_u64().unwrap())}else{0};
        let mut result=card.clone();merge(&mut result,json!({"computeProcesses":own,"idle":stable>=settings["stableSamples"].as_u64().unwrap(),"idleCandidate":candidate,"stableCount":stable}));result
    }).collect::<Vec<_>>();
    let idle_count = cards.iter().filter(|v| v["idle"] == true).count();
    json!({"cards":cards,"idleCount":idle_count,"total":rows.len()})
}

fn squeue_command(username: &str, scope: &str) -> Result<String, String> {
    if scope == "all" {
        return Ok("squeue".into());
    }
    if scope != "mine" {
        return Err("作业查询范围不正确".into());
    }
    if username.is_empty()
        || username.chars().count() > 128
        || username.chars().any(|v| v.is_control())
    {
        return Err("作业查询用户名不正确".into());
    }
    let value = if re(r"^[A-Za-z0-9_][A-Za-z0-9_.@-]*\$?$").is_match(username) {
        username.into()
    } else {
        quote(username)
    };
    Ok(format!("squeue -u {value}"))
}
fn translate_state(code: &str) -> (&'static str, &'static str) {
    let code = code.to_uppercase();
    let code = code.trim_end_matches('+');
    match code {
        "BF" | "BOOT_FAIL" => ("节点启动失败", "negative"),
        "CA" | "CANCELLED" => ("已取消", "neutral"),
        "CD" | "COMPLETED" => ("已完成", "positive"),
        "CF" | "CONFIGURING" => ("准备资源", "pending"),
        "CG" | "COMPLETING" => ("正在收尾", "pending"),
        "DL" | "DEADLINE" => ("超过截止期限", "negative"),
        "F" | "FAILED" => ("执行失败", "negative"),
        "NF" | "NODE_FAIL" => ("节点故障", "negative"),
        "OOM" | "OUT_OF_MEMORY" => ("内存不足", "negative"),
        "PD" | "PENDING" => ("排队等待", "pending"),
        "PR" | "PREEMPTED" => ("已被抢占", "negative"),
        "R" | "RUNNING" => ("运行中", "positive"),
        "RD" | "RESV_DEL_HOLD" => ("预约删除后挂起", "pending"),
        "RF" | "REQUEUE_FED" => ("联邦重新排队", "pending"),
        "RH" | "REQUEUE_HOLD" => ("重新排队并挂起", "pending"),
        "RQ" | "REQUEUED" => ("重新排队", "pending"),
        "RS" | "RESIZING" => ("调整资源规模", "pending"),
        "RV" | "REVOKED" => ("联邦作业已撤回", "neutral"),
        "SI" | "SIGNALING" => ("正在发送信号", "pending"),
        "SE" | "SPECIAL_EXIT" => ("特殊退出后挂起", "pending"),
        "SO" | "STAGE_OUT" => ("正在迁出数据", "pending"),
        "ST" | "STOPPED" => ("已停止（保留资源）", "neutral"),
        "S" | "SUSPENDED" => ("已暂停", "neutral"),
        "TO" | "TIMEOUT" => ("运行超时", "negative"),
        "EXPEDITING" => ("加急调度", "pending"),
        "LAUNCH_FAILED" => ("启动失败", "negative"),
        "POWER_UP_NODE" => ("等待节点启动", "pending"),
        "RECONFIG_FAIL" => ("节点配置失败", "negative"),
        "UPDATE_DB" => ("正在更新作业数据库", "pending"),
        _ => ("未知状态", "neutral"),
    }
}
fn reason_label(code: &str) -> Option<&'static str> {
    Some(match code.to_ascii_lowercase().as_str() {
        "accountingpolicy" => "受计费策略限制",
        "accountnotallowed" => "账户无权使用该分区",
        "associationjoblimit" => "关联配额的作业数已达上限",
        "associationresourcelimit" => "关联配额的资源已达上限",
        "associationtimelimit" => "关联配额的时长已达上限",
        "assocmaxjobslimit" => "关联配额的单用户运行作业数已达上限",
        "assocmaxsubmitjoblimit" => "关联配额的单用户提交作业数已达上限",
        "badconstraints" => "作业资源条件无法满足",
        "begintime" => "尚未到最早启动时间",
        "burstbufferoperation" => "突发缓存操作失败",
        "burstbufferresources" => "突发缓存资源不足",
        "burstbufferstagein" => "正在准备突发缓存数据",
        "cleaning" => "正在清理上次运行",
        "constraints" => "当前无法满足资源条件",
        "deadline" => "无法按截止期限完成",
        "dependency" => "等待依赖作业满足条件",
        "dependencyneversatisfied" => "依赖条件无法满足",
        "fedjoblock" => "等待联邦集群同步锁",
        "inactivelimit" => "超过作业无活动时长限制",
        "invalidaccount" => "作业账户无效",
        "invalidqos" => "服务质量配置无效",
        "jobarraytasklimit" => "作业数组并发任务数已达上限",
        "jobheldadmin" => "管理员挂起作业",
        "jobhelduser" => "用户或账户协调员挂起作业",
        "jobholdmaxrequeue" => "重新排队次数已达上限",
        "joblaunchfailure" => "作业启动失败",
        "licenses" => "等待许可证",
        "maxmemperlimit" => "超过单处理器或单节点内存上限",
        "nodedown" => "所需节点已停机",
        "nonzeroexitcode" => "作业以非零状态退出",
        "none" => "等待本轮调度评估",
        "outofmemory" => "作业内存不足",
        "partitionconfig" => "违反分区配置限制",
        "partitiondown" => "分区已停机",
        "partitioninactive" => "分区未启用",
        "partitionnodelimit" => "不满足分区节点数限制或所需节点不可用",
        "partitiontimelimit" => "超过分区运行时长上限",
        "priority" => "等待更高优先级作业",
        "prolog" => "正在运行作业启动前脚本",
        "qosjoblimit" => "服务质量配额的作业数已达上限",
        "qosresourcelimit" => "服务质量配额的资源已达上限",
        "qostimelimit" => "服务质量配额的时长已达上限",
        "qosusagethreshold" => "未满足服务质量使用量阈值",
        "qosnotallowed" => "无权使用所选服务质量配置",
        "reqnodenotavail" => "指定节点当前不可用",
        "reservation" => "等待预约资源",
        "reservationdeleted" => "所选预约已被删除",
        "resources" => "等待可用资源",
        "scheddefer" => "调度器延后即时资源分配",
        "systemfailure" => "调度系统或基础设施故障",
        "timelimit" => "已用尽运行时长",
        "waitingforscheduling" => "等待调度器确定原因",
        _ => return None,
    })
}
fn resource_name(code: &str) -> Option<&'static str> {
    Some(match code.to_ascii_lowercase().as_str() {
        "bb" => "突发缓存",
        "billing" => "计费资源",
        "cpu" => "处理器",
        "energy" => "能耗",
        "gres" => "通用资源",
        "jobs" | "job" => "运行作业数",
        "license" | "licenses" => "许可证",
        "mem" | "memory" => "内存",
        "node" | "nodes" => "节点数",
        "submitjobs" | "submitjob" => "提交作业数",
        "unknown" => "未识别资源",
        "wall" | "wallduration" => "运行时长",
        _ => return None,
    })
}
fn family_reason(code: &str) -> Option<String> {
    if let Some(c) = re(r"(?i)^(Assoc|QOS)(Grp|Max|Min)(.+)$").captures(code) {
        let owner = if c[1].eq_ignore_ascii_case("Assoc") {
            "关联配额"
        } else {
            "服务质量配额"
        };
        let kind = c[2].to_ascii_lowercase();
        let mut suffix = re(r"(?i)(?:Limit|NotSatisfied)$")
            .replace(&c[3], "")
            .into_owned();
        let mut scope = "";
        if let Some(p) = re(r"(?i)Per(Job|Node|User)$").captures(&suffix) {
            scope = match p[1].to_ascii_lowercase().as_str() {
                "job" => "单作业",
                "node" => "单节点",
                _ => "单用户",
            };
            suffix.truncate(p.get(0).unwrap().start());
        }
        let mut duration = "";
        if re(r"(?i)RunMinutes$").is_match(&suffix) {
            duration = "运行中作业累计时长";
            suffix = re(r"(?i)RunMinutes$").replace(&suffix, "").into_owned();
        } else if re(r"(?i)Minutes$").is_match(&suffix) {
            duration = "累计使用时长";
            suffix = re(r"(?i)Minutes$").replace(&suffix, "").into_owned();
        }
        let resource = resource_name(&suffix)?;
        return Some(match kind.as_str() {
            "grp" => format!(
                "{owner}的{resource}{}已达上限",
                if duration.is_empty() {
                    "总量"
                } else {
                    duration
                }
            ),
            "min" => format!("{resource}请求未达到{owner}下限"),
            _ => format!(
                "{owner}的{scope}{resource}{}超过上限",
                if duration.is_empty() {
                    "请求"
                } else {
                    duration
                }
            ),
        });
    }
    let regex = re(r"(?i)^Max(.+)PerAccount$");
    let c = regex.captures(code)?;
    Some(format!(
        "超过服务质量配额的单账户{}上限",
        resource_name(&c[1])?
    ))
}
fn translate_reason(input: &str) -> String {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0u32;
    for (index, c) in input.char_indices() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(input[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(input[start..].trim());
    parts
        .into_iter()
        .filter(|v| !v.is_empty())
        .map(|part| {
            if let Some(c) = re(r"(?i)^UnavailableNodes\s*:\s*(.*)$").captures(part) {
                return format!("不可用节点：{}", &c[1]);
            }
            let (code, detail) = part
                .split_once(':')
                .map(|(c, d)| (c.trim(), Some(d.trim())))
                .unwrap_or((part.trim(), None));
            let label = reason_label(code)
                .map(str::to_string)
                .or_else(|| family_reason(code));
            match label {
                Some(label) => format!(
                    "{label}{}",
                    detail.map(|d| format!("：{d}")).unwrap_or_default()
                ),
                None => "未知原因".into(),
            }
        })
        .collect::<Vec<_>>()
        .join("；")
}
fn valid_fields(fields: &[String]) -> bool {
    fields.len() == 8
        && re(r"^\d[\w.\[\],:%+_-]*$").is_match(&fields[0])
        && !fields[1].is_empty()
        && !fields[3].is_empty()
        && re(r"^[A-Z][A-Z_+]*$").is_match(&fields[4])
        && re(r"(?i)^(?:(?:\d+-)?\d+:\d{2}(?::\d{2})?|INVALID|UNLIMITED|N/A|-)$")
            .is_match(&fields[5])
        && re(r"^(?:\d+|N/A|-)$").is_match(&fields[6])
}
fn parse_squeue(output: &str) -> Result<Vec<Value>, String> {
    if output.len() > MAX_OUTPUT {
        return Err("squeue 输出格式不正确或超过限制".into());
    }
    const HEADER: [&str; 8] = [
        "JOBID",
        "PARTITION",
        "NAME",
        "USER",
        "ST",
        "TIME",
        "NODES",
        "NODELIST(REASON)",
    ];
    let output = strip_ansi(output);
    let mut jobs = Vec::new();
    let mut columns: Option<Vec<(usize, Option<usize>)>> = None;
    let mut saw_header = false;
    let mut cluster = String::new();
    let tokens = re(r"\S+");
    let loose = re(
        r"(?i)^\s*(\S+)\s+(\S+)\s+(.+?)\s+(\S+)\s+([A-Z][A-Z_+]*)\s+((?:\d+-)?\d+:\d{2}(?::\d{2})?|INVALID|UNLIMITED|N/A|-)\s+(\d+|N/A|-)\s*(.*)$",
    );
    for line in output.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(c) = re(r"(?i)^\s*CLUSTER:\s*(\S+)\s*$").captures(line) {
            cluster = c[1].into();
            columns = None;
            continue;
        }
        let fields = tokens.find_iter(line).collect::<Vec<_>>();
        if fields.len() == 8 && fields.iter().zip(HEADER).all(|(a, b)| a.as_str() == b) {
            columns = Some(
                fields
                    .iter()
                    .enumerate()
                    .map(|(index, t)| {
                        let start = if index == 0 {
                            0
                        } else {
                            line[..fields[index - 1].end()].chars().count() + 1
                        };
                        (
                            start,
                            if index == 7 {
                                None
                            } else {
                                Some(line[..t.end()].chars().count())
                            },
                        )
                    })
                    .collect(),
            );
            saw_header = true;
            continue;
        }
        if re(r"(?i)^\s*squeue:\s*warning\b").is_match(line) {
            continue;
        }
        let Some(ref columns) = columns else {
            if re(r"\bJOBID\b|\bPARTITION\b").is_match(line) {
                return Err("squeue 表头无法识别，请使用默认输出格式".into());
            }
            continue;
        };
        let mut fields = columns
            .iter()
            .map(|(start, end)| {
                line.chars()
                    .skip(*start)
                    .take(end.map(|v| v.saturating_sub(*start)).unwrap_or(usize::MAX))
                    .collect::<String>()
                    .trim()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        if !valid_fields(&fields) {
            fields = loose
                .captures(line)
                .map(|c| {
                    c.iter()
                        .skip(1)
                        .map(|v| v.unwrap().as_str().trim().to_owned())
                        .collect()
                })
                .ok_or("squeue 作业行格式无法识别，请使用默认输出格式")?;
            if !valid_fields(&fields) {
                return Err("squeue 作业行格式无法识别，请使用默认输出格式".into());
            }
        }
        let mut node_list = fields[7].clone();
        let mut reason_code = String::new();
        if let Some(c) = re(r"^(.*?)\s*\((.*)\)\s*$").captures(&fields[7]) {
            node_list = c[1].trim().into();
            reason_code = c[2].trim().into();
        }
        let (label, tone) = translate_state(&fields[4]);
        let mut job = json!({"id":fields[0],"partition":fields[1],"name":fields[2],"user":fields[3],"stateCode":fields[4],"stateLabel":label,"stateTone":tone,"elapsed":fields[5],
          "nodes":fields[6].parse::<u64>().ok(),"nodeList":if node_list=="(null)"{""}else{&node_list},"reasonCode":reason_code,"reasonLabel":translate_reason(&reason_code)});
        if !cluster.is_empty() {
            job["cluster"] = json!(cluster);
        }
        jobs.push(job);
    }
    if !saw_header {
        return Err("squeue 未返回可识别的作业表头".into());
    }
    Ok(jobs)
}
fn slurm_availability(error: &QueryError) -> &'static str {
    if error.exit==Some(127)||re(r"(?i)(?:squeue(?::|\s).*?(?:command not found|not found|未找到|找不到)|command not found.*squeue)").is_match(&error.output){return "unavailable";}
    if re(r"(?i)squeue:|slurm_load_jobs|slurmctld|slurm\.conf|slurm_init|slurm_(?:receive|send|protocol)|permission denied|access denied|invalid user").is_match(&error.output){"available"}else{"unknown"}
}

#[derive(Debug)]
struct JobId {
    base: String,
    kind: &'static str,
    array_task: Option<String>,
    component: Option<String>,
    command_id: String,
}
impl JobId {
    fn info_command(&self) -> String {
        format!(
            "scontrol show job {}",
            literal(if self.kind == "array-group" {
                &self.base
            } else {
                &self.command_id
            })
        )
    }
}
fn literal(value: &str) -> String {
    if value.contains('[') {
        quote(value)
    } else {
        value.into()
    }
}
fn validate_job_id(value: &str) -> Result<JobId, String> {
    if value.len() > 256 {
        return Err("作业 ID 格式不正确".into());
    }
    let number = r"(?:0|[1-9][0-9]{0,9})";
    let range = format!(r"{number}(?:-{number}(?::[1-9][0-9]{{0,9}})?)?");
    let regex = re(&format!(
        r"^([1-9][0-9]{{0,9}})(?:_({number})|_\[({range}(?:,{range})*)(?:%([1-9][0-9]{{0,9}}))?\]|\+({number}))?$"
    ));
    let c = regex
        .captures(value)
        .ok_or("作业 ID 格式不正确，请刷新作业列表")?;
    let kind = if c.get(2).is_some() {
        "array"
    } else if c.get(3).is_some() {
        "array-group"
    } else if c.get(5).is_some() {
        "heterogeneous"
    } else {
        "job"
    };
    if let Some(group) = c.get(3) {
        for part in group.as_str().split(',') {
            let range = part.split(':').next().unwrap();
            if let Some((start, end)) = range.split_once('-') {
                if start.parse::<u64>().unwrap() > end.parse::<u64>().unwrap() {
                    return Err("作业数组范围不正确".into());
                }
            }
        }
    }
    Ok(JobId {
        base: c[1].into(),
        kind,
        array_task: c.get(2).map(|v| v.as_str().into()),
        component: c.get(5).map(|v| v.as_str().into()),
        command_id: if kind == "array-group" {
            format!("{}_[{}]", &c[1], &c[3])
        } else {
            value.into()
        },
    })
}
#[derive(Debug)]
struct JobInfo {
    id: String,
    owner: String,
    uid: String,
    array_job: Option<String>,
    array_task: Option<String>,
    het_job: Option<String>,
    het_offset: Option<String>,
    priority: Option<u64>,
}
fn parse_job_info(output: &str) -> Result<Vec<JobInfo>, String> {
    if output.len() > MAX_OUTPUT {
        return Err("作业详情输出不正确".into());
    }
    let starts = re(r"(?m)^\s*JobId=")
        .find_iter(output)
        .map(|v| v.start())
        .collect::<Vec<_>>();
    if starts.is_empty() {
        return Err("作业不存在，或服务器没有返回可验证的作业详情".into());
    }
    let mut result = Vec::new();
    for (index, start) in starts.iter().enumerate() {
        let block = &output[*start..starts.get(index + 1).copied().unwrap_or(output.len())];
        let ids = re(r"(?m)^\s*JobId=([0-9]+)(?:\s|$)")
            .captures_iter(block)
            .map(|c| c[1].to_owned())
            .collect::<Vec<_>>();
        let owners = re(r"(?m)^\s*UserId=([^\s()]+)\(([0-9]+)\)(?:\s|$)")
            .captures_iter(block)
            .map(|c| (c[1].to_owned(), c[2].to_owned()))
            .collect::<Vec<_>>();
        if ids.len() != 1 || owners.len() != 1 {
            return Err("无法可靠确认作业所属用户".into());
        }
        let field = |name: &str| -> Result<Option<String>, String> {
            let fields = re(&format!(r"(?:^|\s){}=([^\s]+)", regex::escape(name)))
                .captures_iter(block)
                .map(|c| c[1].to_owned())
                .collect::<Vec<_>>();
            if fields.len() > 1 {
                return Err("作业详情字段不明确".into());
            }
            Ok(fields.into_iter().next())
        };
        let priority = field("Priority")?
            .filter(|v| re(r"^[0-9]+$").is_match(v))
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v <= SAFE_INTEGER);
        result.push(JobInfo {
            id: ids[0].clone(),
            owner: owners[0].0.clone(),
            uid: owners[0].1.clone(),
            array_job: field("ArrayJobId")?,
            array_task: field("ArrayTaskId")?,
            het_job: field("HetJobId")?,
            het_offset: field("HetJobOffset")?,
            priority,
        });
    }
    Ok(result)
}
struct OwnedInfo {
    owner: String,
    uid: String,
    priority: Option<u64>,
}
fn assert_owned_info(output: &str, requested: &JobId, username: &str) -> Result<OwnedInfo, String> {
    let records = parse_job_info(output)?;
    if records.iter().any(|v| v.owner != username) {
        return Err("只能删除当前登录用户自己的作业".into());
    }
    if records.iter().map(|v| &v.uid).collect::<HashSet<_>>().len() != 1 {
        return Err("作业所属用户信息不一致".into());
    }
    for record in &records {
        let matches = match requested.kind {
            "job" => {
                record.id == requested.base
                    || record.array_job.as_deref() == Some(&requested.base)
                    || record.het_job.as_deref() == Some(&requested.base)
            }
            "array-group" => record.array_job.as_deref() == Some(&requested.base),
            "array" => {
                record.array_job.as_deref() == Some(&requested.base)
                    && record.array_task == requested.array_task
            }
            "heterogeneous" => {
                record.het_job.as_deref() == Some(&requested.base)
                    && record.het_offset == requested.component
            }
            _ => false,
        };
        if !matches {
            return Err("服务器返回的作业 ID 与所选作业不一致".into());
        }
    }
    let first = &records[0];
    Ok(OwnedInfo {
        owner: first.owner.clone(),
        uid: first.uid.clone(),
        priority: if records.iter().all(|v| v.priority == first.priority) {
            first.priority
        } else {
            None
        },
    })
}
fn parse_priorities(output: &str) -> Result<Vec<u64>, String> {
    let lines = output
        .trim()
        .lines()
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() || lines.len() > 100_000 {
        return Err("服务器没有返回作业权重".into());
    }
    let regex = re(r"^Priority=([0-9]+)$");
    lines
        .iter()
        .map(|line| {
            regex
                .captures(line.trim())
                .and_then(|c| c[1].parse::<u64>().ok())
                .filter(|v| *v <= SAFE_INTEGER)
                .ok_or_else(|| "作业权重输出格式不正确".into())
        })
        .collect()
}

fn home_path(username: &str) -> Result<String, String> {
    if username.is_empty()
        || username.chars().count() > 80
        || [".", ".."].contains(&username)
        || username
            .chars()
            .any(|v| v == '/' || v == '\\' || v.is_control())
    {
        return Err("用户名无法用于个人目录统计".into());
    }
    Ok(format!("/home/{username}"))
}
fn valid_uid(uid: u64, root: bool) -> Result<(), String> {
    if uid > u32::MAX as u64 || (!root && uid == 0) {
        Err("用户 UID 不正确或不允许清理 root 的临时文件".into())
    } else {
        Ok(())
    }
}
const UID_HEADER: &str = "uid=$(id -u) || exit 1\ncase \"$uid\" in ''|*[!0-9]*) exit 1 ;; esac";
fn scan_row(label: &str, root: &str, follow_root: bool) -> String {
    let output = r#"-uid "$uid" -printf 'ENTRY\t%D\t%i\t%b\t%y\n'"#;
    // Own files inside shared directories count. Other users' inaccessible
    // directories are pruned; symlinks and mount points are never followed.
    let predicate = if root == "/tmp" {
        format!(
            r#"\( -type d ! -uid "$uid" \( ! -readable -o ! -executable \) -prune \) -o \( {output} \)"#
        )
    } else {
        output.into()
    };
    let header=format!("printf '{label}\\t'\n{{ find {} {} -xdev -mindepth 1 {predicate}; printf 'EXIT\\t%s\\n' \"$?\"; }}",if follow_root{"-H"}else{"-P"},quote(root));
    format!(
        "{header}{}",
        r#" | awk -F '\t' '
$1 == "ENTRY" { if (!seen[$2 ":" $3]++) bytes += $4 * 512; entries++; if ($5 == "f" || $5 == "l") candidates++ }
$1 == "EXIT" { code = $2; finished = 1 }
END { printf "%.0f\t%d\t%d\t%d\n", bytes, entries, candidates, finished ? code : 99 }'"#
    )
}
fn build_storage_scan(username: &str) -> Result<String, String> {
    Ok(format!(
        "# Lion storage scan v1\n{UID_HEADER}\nprintf 'LION_STORAGE_V1\\t%s\\n' \"$uid\"\n{}\n{}",
        scan_row("HOME", &home_path(username)?, true),
        scan_row("TMP", "/tmp", false)
    ))
}
fn build_tmp_preview() -> String {
    format!("# Lion tmp preview v1\n{UID_HEADER}\n{}\nprintf 'LION_TMP_PREVIEW_V1\\t%s\\n' \"$uid\"\n{}",TMP_ROOT_CHECK,scan_row("TMP","/tmp",false))
}
const TMP_ROOT_CHECK: &str = r#"if [ -L /tmp ] || [ ! -d /tmp ]; then printf '临时目录不是可安全清理的实际目录\n' >&2; exit 1; fi"#;
fn cleanup_round(directories_only: bool) -> String {
    let types = if directories_only {
        "-type d -empty"
    } else {
        r"\( -type f -o -type l \)"
    };
    let remove=format!("uid=\"$1\"; shift\nfind -P \"$@\" -maxdepth 0 -uid \"$uid\" {types} -delete -printf 'DELETED\\n'");
    let body = format!(
        r#"{{ find -P /tmp -xdev -mindepth 1 \( -type d ! -uid "$uid" \( ! -readable -o ! -executable \) -prune \) -o \( -uid "$uid" {types} -execdir /bin/sh -c {} lion-tmp "$uid" {{}} + -prune \); printf 'EXIT\t%s\n' "$?"; }}"#,
        quote(&remove)
    );
    format!(
        "{body}{}",
        r#" | awk -F '\t' '
$1 == "DELETED" { count++ }
$1 == "EXIT" { code = $2; finished = 1 }
END { printf "%d %d\n", count, finished ? code : 99 }'"#
    )
}
fn build_tmp_cleanup(uid: u64) -> Result<String, String> {
    valid_uid(uid, false)?;
    let header=format!("# Lion tmp cleanup v1\nPATH=/usr/bin:/bin\nexport PATH\n{UID_HEADER}\nif [ \"$uid\" != '{uid}' ] || [ \"$uid\" = 0 ]; then printf '用户 UID 已变化或禁止清理 root 临时文件\\n' >&2; exit 1; fi\n{TMP_ROOT_CHECK}\nprintf 'LION_TMP_CLEANUP_V1\\t%s\\n' \"$uid\"\ntotal=0\nresult=$({})\n",cleanup_round(false));
    let tail = r#"case "$result" in ''|*[!0-9' ']*) exit 1 ;; esac
set -- $result
if [ "$#" != 2 ]; then exit 1; fi
total=$1
status=$2
round=0
if [ "$status" = 0 ]; then
  while [ "$round" -lt 128 ]; do
    round=$((round + 1))
    if [ -L /tmp ] || [ ! -d /tmp ]; then status=1; break; fi
    result=$(__DIRECTORIES__)
    case "$result" in ''|*[!0-9' ']*) exit 1 ;; esac
    set -- $result
    if [ "$#" != 2 ]; then exit 1; fi
    total=$((total + $1))
    status=$2
    if [ "$status" != 0 ] || [ "$1" = 0 ]; then break; fi
    if [ "$round" = 128 ]; then status=2; fi
  done
fi
printf 'RESULT\t%s\t%s\n' "$total" "$status""#;
    Ok(header + &tail.replace("__DIRECTORIES__", &cleanup_round(true)))
}
fn numeric(value: &str) -> Result<u64, String> {
    if !re(r"^(?:0|[1-9][0-9]*)$").is_match(value) {
        return Err("空间统计输出格式不正确".into());
    }
    value
        .parse::<u64>()
        .ok()
        .filter(|v| *v <= SAFE_INTEGER)
        .ok_or_else(|| "空间统计数值超出范围".into())
}
fn storage_rows(
    output: &str,
    marker: &str,
    labels: &[&str],
) -> Result<(u64, Vec<Vec<u64>>), String> {
    if output.len() > 4096 {
        return Err("空间统计输出不正确".into());
    }
    let mut lines = output.trim().lines();
    let header = lines.next().unwrap_or("").split('\t').collect::<Vec<_>>();
    if header.len() != 2 || header[0] != marker {
        return Err("空间统计输出不完整".into());
    }
    let uid = numeric(header[1])?;
    valid_uid(uid, true)?;
    let mut data = Vec::new();
    for label in labels {
        let fields = lines
            .next()
            .ok_or("空间统计输出不完整")?
            .split('\t')
            .collect::<Vec<_>>();
        if fields[0] != *label || fields.len() != if *label == "RESULT" { 3 } else { 5 } {
            return Err("空间统计输出格式不正确".into());
        }
        data.push(
            fields[1..]
                .iter()
                .map(|v| numeric(v))
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    if lines.next().is_some() {
        return Err("空间统计输出不完整".into());
    }
    Ok((uid, data))
}
fn scan_result(values: &[u64]) -> Result<(Option<u64>, u64, bool), String> {
    let bytes = values[0];
    let entries = values[1];
    let files = values[2];
    let exit = values[3];
    match exit {
        127 => return Err("服务器缺少 GNU find，无法统计个人空间".into()),
        126 => return Err("服务器不允许执行空间统计工具".into()),
        256.. => return Err("空间统计返回了无效退出码".into()),
        _ => {}
    }
    if files > entries {
        return Err("空间统计文件数量不正确".into());
    }
    Ok((
        if exit != 0 && entries == 0 {
            None
        } else {
            Some(bytes)
        },
        files,
        exit != 0,
    ))
}
fn parse_storage_scan(output: &str, username: &str) -> Result<Value, String> {
    let (uid, rows) = storage_rows(output, "LION_STORAGE_V1", &["HOME", "TMP"])?;
    let (home, _, home_partial) = scan_result(&rows[0])?;
    let (tmp, _, tmp_partial) = scan_result(&rows[1])?;
    let total = match (home, tmp) {
        (Some(a), Some(b)) => Some(
            a.checked_add(b)
                .filter(|v| *v <= SAFE_INTEGER)
                .ok_or("空间占用合计超出范围")?,
        ),
        _ => None,
    };
    let mut warnings = Vec::new();
    if home_partial {
        warnings.push("个人目录部分内容无法读取，统计可能不完整");
    }
    if tmp_partial {
        warnings.push("/tmp 部分目录无法读取，统计仅包括当前可访问内容");
    }
    Ok(
        json!({"uid":uid,"homePath":home_path(username)?,"homeBytes":home,"tmpBytes":tmp,"totalBytes":total,"partial":home_partial||tmp_partial,"warnings":warnings}),
    )
}
fn parse_tmp_preview(output: &str) -> Result<Value, String> {
    let (uid, rows) = storage_rows(output, "LION_TMP_PREVIEW_V1", &["TMP"])?;
    let (bytes, count, partial) = scan_result(&rows[0])?;
    Ok(
        json!({"uid":uid,"bytes":bytes,"fileCount":count,"partial":partial,"warnings":if partial{vec!["/tmp 部分目录无法读取，只清理当前可访问的本人文件"]}else{vec![]}}),
    )
}
fn parse_tmp_cleanup(output: &str) -> Result<Value, String> {
    let (uid, rows) = storage_rows(output, "LION_TMP_CLEANUP_V1", &["RESULT"])?;
    let count = rows[0][0];
    let partial = rows[0][1] != 0;
    Ok(
        json!({"uid":uid,"deletedCount":count,"partial":partial,"cleanupStatus":if partial{"partial"}else{"complete"},"warnings":if partial{vec!["部分文件无法清理，可能存在权限限制或文件正在变化"]}else{vec![]}}),
    )
}

pub fn blank_gpu(options: &Value) -> Value {
    json!({"status":"unknown","availability":"unknown","availabilityReason":null,"availabilityCheckedAt":null,
      "cards":[],"idleCount":0,"total":0,"lastUpdated":null,"nextPollAt":null,"intervalSec":normalize_gpu(options)["intervalSec"],"initialConfirming":false})
}
pub fn blank_slurm(scope: &str) -> Value {
    json!({"availability":"unknown","status":"unknown","scope":if scope=="all" {"all"} else {"mine"},"jobs":[],
        "lastUpdated":null,"nextPollAt":null,"intervalSec":30,"error":null,"availabilityReason":null})
}
pub fn blank_storage(username: &str) -> Value {
    json!({"status":"unknown","homePath":home_path(username).unwrap_or_default(),"homeBytes":null,"tmpBytes":null,"totalBytes":null,
      "lastUpdated":null,"nextPollAt":null,"intervalSec":300,"error":null,"warnings":[],"partial":false})
}

#[derive(Clone, Debug)]
struct QueryError {
    message: String,
    output: String,
    exit: Option<u32>,
    gpu_rows: Option<Vec<Value>>,
    detected: bool,
}
impl QueryError {
    fn new(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            output: message.clone(),
            message,
            exit: None,
            gpu_rows: None,
            detected: false,
        }
    }
}
#[derive(Clone)]
struct Confirmation {
    purpose: &'static str,
    job_id: String,
    owner: String,
    uid: String,
    expires: u64,
    epoch: u64,
}

pub struct Monitor {
    profile: Mutex<Value>,
    executor: Arc<dyn Executor>,
    events: Events,
    cancel: CancellationToken,
    started: AtomicBool,
    gpu: Mutex<Value>,
    slurm: Mutex<Value>,
    storage: Mutex<Value>,
    gpu_gate: Mutex<()>,
    slurm_gate: Mutex<()>,
    storage_gate: Mutex<()>,
    job_gate: Mutex<()>,
    storage_action_gate: Mutex<()>,
    gpu_notify: Notify,
    slurm_notify: Notify,
    storage_notify: Notify,
    gpu_epoch: AtomicU64,
    slurm_epoch: AtomicU64,
    summary_mode: AtomicBool,
    initial_remaining: AtomicU32,
    tokens: Mutex<HashMap<String, Confirmation>>,
    slurm_enabled: bool,
    storage_enabled: bool,
}
impl Monitor {
    pub fn new(profile: Value, executor: Arc<dyn Executor>, events: Events) -> Arc<Self> {
        let options = normalize_gpu(&profile["gpu"]);
        let username = profile["username"].as_str().unwrap_or("");
        let slurm_enabled = std::env::var("LION_SLURM_DISABLED").ok().as_deref() != Some("1");
        Arc::new(Self {
            gpu: Mutex::new(blank_gpu(&options)),
            slurm: Mutex::new(blank_slurm("mine")),
            storage: Mutex::new(blank_storage(username)),
            initial_remaining: AtomicU32::new(
                options["stableSamples"]
                    .as_u64()
                    .unwrap_or(2)
                    .saturating_sub(1) as u32,
            ),
            profile: Mutex::new(profile),
            executor,
            events,
            cancel: CancellationToken::new(),
            started: AtomicBool::new(false),
            gpu_gate: Mutex::new(()),
            slurm_gate: Mutex::new(()),
            storage_gate: Mutex::new(()),
            job_gate: Mutex::new(()),
            storage_action_gate: Mutex::new(()),
            gpu_notify: Notify::new(),
            slurm_notify: Notify::new(),
            storage_notify: Notify::new(),
            gpu_epoch: AtomicU64::new(0),
            slurm_epoch: AtomicU64::new(0),
            summary_mode: AtomicBool::new(false),
            tokens: Mutex::new(HashMap::new()),
            slurm_enabled,
            storage_enabled: slurm_enabled
                && std::env::var("LION_STORAGE_DISABLED").ok().as_deref() != Some("1"),
        })
    }
    pub fn start(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        for kind in ["gpu", "slurm", "storage"] {
            let monitor = self.clone();
            tokio::spawn(async move {
                monitor.run_loop(kind).await;
            });
        }
    }
    async fn run_loop(self: Arc<Self>, kind: &'static str) {
        match kind {
            "gpu" => self.poll_gpu(false).await,
            "slurm" => self.poll_slurm().await,
            _ => self.poll_storage().await,
        }
        loop {
            if self.cancel.is_cancelled() {
                return;
            }
            let (at, notify) = match kind {
                "gpu" => (
                    self.gpu.lock().await["nextPollAt"].as_u64(),
                    &self.gpu_notify,
                ),
                "slurm" => (
                    self.slurm.lock().await["nextPollAt"].as_u64(),
                    &self.slurm_notify,
                ),
                _ => (
                    self.storage.lock().await["nextPollAt"].as_u64(),
                    &self.storage_notify,
                ),
            };
            let delay =
                Duration::from_millis(at.map(|v| v.saturating_sub(now())).unwrap_or(86_400_000));
            tokio::select! { biased;
                _=self.cancel.cancelled()=>return,
                _=notify.notified()=>continue,
                _=tokio::time::sleep(delay)=>{}
            }
            if at.is_none() {
                continue;
            }
            match kind {
                "gpu" => self.poll_gpu(false).await,
                "slurm" => self.poll_slurm().await,
                _ => self.poll_storage().await,
            }
        }
    }
    pub async fn stop(&self) {
        self.cancel.cancel();
        self.gpu_epoch.fetch_add(1, Ordering::SeqCst);
        self.slurm_epoch.fetch_add(1, Ordering::SeqCst);
        self.tokens.lock().await.clear();
    }
    fn connected(&self) -> Result<(), String> {
        if self.cancel.is_cancelled() {
            Err("SSH 未连接或连接已变化，请重新确认".into())
        } else {
            Ok(())
        }
    }
    fn emit(&self, kind: &str, value: &Value) {
        if !self.cancel.is_cancelled() {
            (self.events)(kind, value.clone());
        }
    }
    fn event(&self, level: &str, message: &str) {
        self.emit("event",&json!({"id":uuid::Uuid::new_v4().to_string(),"time":now(),"level":level,"message":message}));
    }
    async fn exec(&self, command: &str, storage: bool) -> Result<String, QueryError> {
        self.connected().map_err(QueryError::new)?;
        let timeout = Duration::from_secs(if storage { 120 } else { 8 });
        let result = tokio::select! {biased; _=self.cancel.cancelled()=>return Err(QueryError::new("SSH 连接已断开")),result=self.executor.exec(command,timeout,if storage {4096}else{MAX_OUTPUT})=>result};
        match result {
            Ok(output) if output.exit_code == Some(0) => Ok(output.stdout),
            Ok(output) => {
                let message = if output.stderr.trim().is_empty() {
                    format!(
                        "查询失败（退出码 {}）",
                        output
                            .exit_code
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "未知".into())
                    )
                } else {
                    limited(output.stderr.trim())
                };
                Err(QueryError {
                    output: format!("{}\n{}\n{}", output.stdout, output.stderr, message),
                    message,
                    exit: output.exit_code,
                    gpu_rows: None,
                    detected: false,
                })
            }
            Err(error) => Err(QueryError::new(error)),
        }
    }
    pub async fn refresh_gpu(&self) {
        self.poll_gpu(true).await;
        self.gpu_notify.notify_one();
    }
    pub async fn refresh_slurm(&self) {
        self.poll_slurm().await;
        self.slurm_notify.notify_one();
    }
    pub async fn refresh_storage(&self) {
        self.poll_storage().await;
        self.storage_notify.notify_one();
    }
    pub async fn update_gpu(&self, options: Value) -> Result<(), String> {
        self.connected()?;
        let options = normalize_gpu(&options);
        self.gpu_epoch.fetch_add(1, Ordering::SeqCst);
        self.initial_remaining.store(0, Ordering::SeqCst);
        self.profile.lock().await["gpu"] = options.clone();
        {
            let mut state = self.gpu.lock().await;
            merge(
                &mut state,
                json!({"intervalSec":options["intervalSec"],"status":"unknown","initialConfirming":false,"nextPollAt":null}),
            );
            if let Some(cards) = state["cards"].as_array_mut() {
                for card in cards {
                    card["stableCount"] = json!(0);
                    card["idle"] = json!(false);
                }
            }
            self.emit("gpu", &state);
        }
        if options["enabled"] == true {
            self.poll_gpu(false).await;
        }
        self.gpu_notify.notify_one();
        Ok(())
    }
    pub async fn set_slurm_scope(&self, scope: &str) -> Result<(), String> {
        self.initialize_slurm_scope(scope).await?;
        self.refresh_slurm().await;
        Ok(())
    }
    /// Set the initial scope before starting the independent query task. This
    /// never waits for a remote command, so disconnect remains responsive.
    pub async fn initialize_slurm_scope(&self, scope: &str) -> Result<(), String> {
        self.connected()?;
        if !self.slurm_enabled {
            return Err("当前连接已禁用 Slurm 作业查询".into());
        }
        if !["mine", "all"].contains(&scope) {
            return Err("作业范围应为个人或全部".into());
        }
        {
            let mut state = self.slurm.lock().await;
            if state["scope"] != scope {
                self.slurm_epoch.fetch_add(1, Ordering::SeqCst);
                *state = blank_slurm(scope);
                state["status"] = json!("loading");
                self.emit("slurm", &state);
                self.tokens
                    .lock()
                    .await
                    .retain(|_, v| v.purpose != "cancel");
            }
        }
        Ok(())
    }
    async fn poll_gpu(&self, force: bool) {
        let _gate = self.gpu_gate.lock().await;
        let options = normalize_gpu(&self.profile.lock().await["gpu"]);
        if self.connected().is_err() || (!force && options["enabled"] == false) {
            return;
        }
        let epoch = self.gpu_epoch.load(Ordering::SeqCst);
        {
            let mut state = self.gpu.lock().await;
            state["status"] = json!("loading");
            state["nextPollAt"] = Value::Null;
            self.emit("gpu", &state);
        }
        let result = self.read_gpu().await;
        if self.cancel.is_cancelled() || epoch != self.gpu_epoch.load(Ordering::SeqCst) {
            return;
        }
        let mut state = self.gpu.lock().await;
        match result {
            Ok((rows, processes)) => {
                let summary = summarize_gpu(
                    &rows,
                    &processes,
                    &options,
                    state["cards"].as_array().map(Vec::as_slice).unwrap_or(&[]),
                );
                let initial = self.initial_remaining.load(Ordering::SeqCst) > 0
                    && summary["cards"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|v| v["idleCandidate"] == true && v["idle"] == false);
                *state = summary;
                merge(
                    &mut state,
                    json!({"status":"ready","lastUpdated":now(),"nextPollAt":null,"intervalSec":options["intervalSec"],
                  "queryMode":if self.summary_mode.load(Ordering::SeqCst){"summary"}else{"csv"},"error":null,"availability":"available","availabilityReason":null,"availabilityCheckedAt":now(),"initialConfirming":initial}),
                );
            }
            Err(error) => {
                self.initial_remaining.store(0, Ordering::SeqCst);
                let availability = gpu_availability(&error);
                if availability != "unknown" || state["availability"] != "available" {
                    state["availability"] = json!(availability);
                }
                merge(
                    &mut state,
                    json!({"status":"error","error":limited(&error.message),"initialConfirming":false,"availabilityReason":gpu_reason(&error,availability),"availabilityCheckedAt":now()}),
                );
                if let Some(mut rows) = error.gpu_rows {
                    if state["availability"] == "available" {
                        for row in &mut rows {
                            row["processesKnown"] = json!(false);
                        }
                        merge(&mut state, summarize_gpu(&rows, &[], &options, &[]));
                    }
                }
                if ["absent", "unavailable"]
                    .iter()
                    .any(|v| state["availability"] == *v)
                {
                    merge(
                        &mut state,
                        json!({"cards":[],"idleCount":0,"total":0,"lastUpdated":null}),
                    );
                }
                if let Some(cards) = state["cards"].as_array_mut() {
                    for card in cards {
                        card["stableCount"] = json!(0);
                    }
                }
            }
        }
        if options["enabled"] == true {
            let interval = if state["initialConfirming"] == true
                && self.initial_remaining.load(Ordering::SeqCst) > 0
            {
                self.initial_remaining.fetch_sub(1, Ordering::SeqCst);
                250
            } else {
                self.initial_remaining.store(0, Ordering::SeqCst);
                state["initialConfirming"] = json!(false);
                if ["absent", "unavailable"]
                    .iter()
                    .any(|v| state["availability"] == *v)
                {
                    60_000
                } else {
                    options["intervalSec"].as_u64().unwrap_or(5) * 1000
                }
            };
            state["nextPollAt"] = json!(now() + interval);
        } else {
            state["nextPollAt"] = Value::Null;
        }
        self.emit("gpu", &state);
    }
    async fn read_summary(&self) -> Result<(Vec<Value>, Vec<Value>), QueryError> {
        let output = self.exec(SUMMARY_QUERY, false).await?;
        let (rows, processes, complete) = parse_summary(&output).map_err(QueryError::new)?;
        if !complete {
            let mut e = QueryError::new("nvidia-smi 进程表不完整，无法确认显卡是否空闲");
            e.detected = true;
            e.gpu_rows = Some(rows);
            return Err(e);
        }
        Ok((rows, processes))
    }
    async fn read_gpu(&self) -> Result<(Vec<Value>, Vec<Value>), QueryError> {
        if self.summary_mode.load(Ordering::SeqCst) {
            return self.read_summary().await;
        }
        let mut known_rows = None;
        let csv_result = async {
            let rows =
                parse_gpu_rows(&self.exec(GPU_QUERY, false).await?).map_err(QueryError::new)?;
            known_rows = Some(rows.clone());
            let processes = parse_compute_rows(&self.exec(COMPUTE_QUERY, false).await?)
                .map_err(QueryError::new)?;
            Ok((rows, processes))
        }
        .await;
        match csv_result {
            Ok(value) => Ok(value),
            Err(error) => {
                let capability = gpu_availability(&error);
                if self.cancel.is_cancelled()
                    || capability == "absent"
                    || (capability == "unavailable" && known_rows.is_none())
                {
                    return Err(error);
                }
                self.summary_mode.store(true, Ordering::SeqCst);
                match self.read_summary().await {
                    Ok(value) => {
                        self.event("info", "服务器仅支持状态输出，已使用兼容模式");
                        Ok(value)
                    }
                    Err(mut error) => {
                        if known_rows.is_some() && gpu_availability(&error) == "unknown" {
                            error.detected = true;
                            if error.gpu_rows.is_none() {
                                error.gpu_rows = known_rows;
                            }
                        }
                        Err(error)
                    }
                }
            }
        }
    }
    async fn poll_slurm(&self) {
        let _gate = self.slurm_gate.lock().await;
        if self.connected().is_err() || !self.slurm_enabled {
            return;
        }
        let epoch = self.slurm_epoch.load(Ordering::SeqCst);
        let scope;
        {
            let mut state = self.slurm.lock().await;
            scope = text(&state, "scope");
            state["status"] = json!("loading");
            state["nextPollAt"] = Value::Null;
            self.emit("slurm", &state);
        }
        let username = text(&*self.profile.lock().await, "username");
        let command = squeue_command(&username, &scope);
        let mut query_succeeded = false;
        let result = match command {
            Ok(command) => match self.exec(&command, false).await {
                Ok(output) => {
                    query_succeeded = true;
                    parse_squeue(&output).map_err(QueryError::new)
                }
                Err(e) => Err(e),
            },
            Err(e) => Err(QueryError::new(e)),
        };
        if self.cancel.is_cancelled() || epoch != self.slurm_epoch.load(Ordering::SeqCst) {
            return;
        }
        let mut state = self.slurm.lock().await;
        match result {
            Ok(jobs) => {
                let first = state["availability"] != "available";
                merge(
                    &mut state,
                    json!({"availability":"available","status":"ready","jobs":jobs,"lastUpdated":now(),"error":null,"availabilityReason":null}),
                );
                if first {
                    self.event("info", "Slurm 作业查询已启用");
                }
            }
            Err(error) => {
                let availability = if query_succeeded {
                    "available"
                } else {
                    slurm_availability(&error)
                };
                if availability != "unknown" || state["availability"] != "available" {
                    state["availability"] = json!(availability);
                }
                let message = if availability == "unavailable" {
                    "未找到 squeue，Slurm 作业监测不可用".to_string()
                } else {
                    limited(&error.message)
                };
                merge(
                    &mut state,
                    json!({"status":"error","error":message,"availabilityReason":message}),
                );
                if state["availability"] == "unavailable" {
                    state["jobs"] = json!([]);
                    state["lastUpdated"] = Value::Null;
                }
            }
        }
        state["nextPollAt"] = json!(
            now()
                + if state["availability"] == "unavailable" {
                    60_000
                } else {
                    30_000
                }
        );
        self.emit("slurm", &state);
    }
    async fn poll_storage(&self) {
        let _gate = self.storage_gate.lock().await;
        if self.connected().is_err() || !self.storage_enabled {
            return;
        }
        {
            let mut state = self.storage.lock().await;
            merge(&mut state, json!({"status":"loading","nextPollAt":null}));
            self.emit("storage", &state);
        }
        let _action_gate = self.storage_action_gate.lock().await;
        let username = text(&*self.profile.lock().await, "username");
        let result = match build_storage_scan(&username) {
            Ok(command) => self
                .exec(&command, true)
                .await
                .map_err(|e| e.message)
                .and_then(|output| parse_storage_scan(&output, &username)),
            Err(e) => Err(e),
        };
        if self.cancel.is_cancelled() {
            return;
        }
        let mut state = self.storage.lock().await;
        match result {
            Ok(result) => {
                merge(&mut state, result);
                merge(
                    &mut state,
                    json!({"status":"ready","error":null,"lastUpdated":now()}),
                );
            }
            Err(error) => merge(
                &mut state,
                json!({"status":"error","error":limited(&error)}),
            ),
        }
        state["nextPollAt"] = json!(now() + 300_000);
        self.emit("storage", &state);
    }
    async fn check_job(&self, job_id: &str, epoch: u64) -> Result<(), String> {
        self.connected()?;
        if !self.slurm_enabled {
            return Err("SSH 未连接或 Slurm 操作已禁用".into());
        }
        if epoch != self.slurm_epoch.load(Ordering::SeqCst) {
            return Err("连接已变化，请刷新作业列表后重试".into());
        }
        let state = self.slurm.lock().await;
        if state["availability"] != "available"
            || !state["error"].is_null()
            || state["lastUpdated"]
                .as_u64()
                .map(|v| now().saturating_sub(v) > 90_000)
                .unwrap_or(true)
        {
            return Err("作业列表已过期，请先刷新".into());
        }
        let rows = state["jobs"]
            .as_array()
            .ok_or("请从当前服务器的最新作业列表中选择作业")?
            .iter()
            .filter(|row| row["id"] == job_id)
            .collect::<Vec<_>>();
        if rows.len() != 1 || rows[0]["cluster"].as_str().is_some_and(|v| !v.is_empty()) {
            return Err("请从当前服务器的最新作业列表中选择作业".into());
        }
        Ok(())
    }
    async fn token(&self, confirmation: Confirmation, max: usize) -> String {
        let mut tokens = self.tokens.lock().await;
        tokens.retain(|_, v| v.expires > now());
        if tokens.len() >= max {
            if let Some(key) = tokens
                .iter()
                .min_by_key(|(_, v)| v.expires)
                .map(|(k, _)| k.clone())
            {
                tokens.remove(&key);
            }
        }
        let token = uuid::Uuid::new_v4().to_string();
        tokens.insert(token.clone(), confirmation);
        token
    }
    pub async fn action(&self, method: &str, args: &[Value]) -> Result<Value, String> {
        self.connected()?;
        match method {
            "prepare-job-cancel" | "cancel-job" | "get-job-priority" => {
                self.job_action(method, args).await
            }
            "prepare-tmp-cleanup" | "cleanup-tmp" => self.storage_action(method, args).await,
            _ => Err("不支持的远程操作".into()),
        }
    }
    async fn job_action(&self, method: &str, args: &[Value]) -> Result<Value, String> {
        let job_id = args
            .first()
            .and_then(Value::as_str)
            .ok_or("作业 ID 格式不正确")?;
        let id = validate_job_id(job_id)?;
        let epoch = self.slurm_epoch.load(Ordering::SeqCst);
        self.check_job(job_id, epoch).await?;
        let _gate = self.job_gate.lock().await;
        self.check_job(job_id, epoch).await?;
        let username = text(&*self.profile.lock().await, "username");
        if method == "get-job-priority" {
            let command = format!("{} | grep -o 'Priority=[0-9]*'", id.info_command());
            let output = self.exec(&command, false).await.map_err(|e| e.message)?;
            self.check_job(job_id, epoch).await?;
            let priorities = parse_priorities(&output)?;
            let priority = if priorities.iter().all(|v| *v == priorities[0]) {
                Some(priorities[0])
            } else {
                None
            };
            return Ok(json!({"jobId":job_id,"priority":priority,"priorities":priorities}));
        }
        let prepared = if method == "cancel-job" {
            // Consume before any await or mutation: a network failure cannot replay cancellation.
            let token = args.get(1).and_then(Value::as_str).unwrap_or("");
            let entry = self
                .tokens
                .lock()
                .await
                .remove(token)
                .ok_or("删除确认已失效，请重新确认")?;
            if entry.purpose != "cancel"
                || entry.job_id != job_id
                || entry.owner != username
                || entry.expires <= now()
                || entry.epoch != epoch
            {
                return Err("删除确认已失效，请重新确认".into());
            }
            Some(entry)
        } else {
            None
        };
        let output = self
            .exec(&id.info_command(), false)
            .await
            .map_err(|e| e.message)?;
        self.check_job(job_id, epoch).await?;
        let owned = assert_owned_info(&output, &id, &username)?;
        if let Some(prepared) = prepared {
            if owned.uid != prepared.uid || owned.owner != prepared.owner {
                return Err("作业所属用户已变化，请重新确认".into());
            }
            self.exec(&format!("scancel {}", literal(&id.command_id)), false)
                .await
                .map_err(|e| e.message)?;
            self.connected()?;
            self.event("info", &format!("已请求取消自己的作业 {job_id}"));
            self.refresh_slurm().await;
            Ok(json!({"jobId":job_id,"cancelled":true}))
        } else {
            let expires = now() + TOKEN_LIFETIME;
            let token = self
                .token(
                    Confirmation {
                        purpose: "cancel",
                        job_id: job_id.into(),
                        owner: owned.owner.clone(),
                        uid: owned.uid,
                        expires,
                        epoch,
                    },
                    64,
                )
                .await;
            let mut value =
                json!({"jobId":job_id,"owner":owned.owner,"token":token,"expiresAt":expires});
            if let Some(priority) = owned.priority {
                value["priority"] = json!(priority);
            }
            Ok(value)
        }
    }
    async fn storage_action(&self, method: &str, args: &[Value]) -> Result<Value, String> {
        if !self.storage_enabled {
            return Err("SSH 未连接或空间操作已禁用".into());
        }
        let _gate = self.storage_action_gate.lock().await;
        self.connected()?;
        let username = text(&*self.profile.lock().await, "username");
        if method == "prepare-tmp-cleanup" {
            let output = self
                .exec(&build_tmp_preview(), true)
                .await
                .map_err(|e| e.message)?;
            self.connected()?;
            let mut preview = parse_tmp_preview(&output)?;
            let uid = preview["uid"].as_u64().ok_or("用户 UID 不正确")?;
            valid_uid(uid, false)?;
            if preview["bytes"].is_null() {
                return Err("无法读取临时目录，暂不能确认清理范围".into());
            }
            let expires = now() + TOKEN_LIFETIME;
            let token = self
                .token(
                    Confirmation {
                        purpose: "cleanup",
                        job_id: String::new(),
                        owner: username.clone(),
                        uid: uid.to_string(),
                        expires,
                        epoch: 0,
                    },
                    32,
                )
                .await;
            merge(
                &mut preview,
                json!({"username":username,"root":"/tmp","token":token,"expiresAt":expires}),
            );
            Ok(preview)
        } else {
            let token = args.first().and_then(Value::as_str).unwrap_or("");
            let prepared = self
                .tokens
                .lock()
                .await
                .remove(token)
                .ok_or("清理确认已失效，请重新确认")?;
            if prepared.purpose != "cleanup"
                || prepared.owner != username
                || prepared.expires <= now()
            {
                return Err("清理确认已失效，请重新确认".into());
            }
            let uid = prepared.uid.parse::<u64>().map_err(|_| "用户 UID 不正确")?;
            let output = self
                .exec(&build_tmp_cleanup(uid)?, true)
                .await
                .map_err(|e| e.message)?;
            self.connected()?;
            let result = parse_tmp_cleanup(&output)?;
            if result["uid"] != uid {
                return Err("服务器返回的用户 UID 不一致，请手动检查清理结果".into());
            }
            let partial = result["partial"] == true;
            self.event(
                if partial { "warning" } else { "info" },
                &format!(
                    "已清理自己的 /tmp 文件及空目录，共 {} 项{}",
                    result["deletedCount"],
                    if partial {
                        "（部分内容未清理）"
                    } else {
                        ""
                    }
                ),
            );
            // Wake the independent scanner after releasing this serialized operation.
            self.storage.lock().await["nextPollAt"] = json!(now());
            self.storage_notify.notify_one();
            Ok(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::CommandOutput;

    fn options() -> Value {
        json!({"stableSamples":2})
    }
    fn slurm_row(values: &[&str]) -> String {
        let widths = [18, 9, 8, 8, 2, 10, 6];
        values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                if i < widths.len() {
                    format!("{:>width$}", v, width = widths[i])
                } else {
                    v.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
    fn queue() -> String {
        format!(
            "{}\n{}\n{}\n",
            slurm_row(&[
                "JOBID",
                "PARTITION",
                "NAME",
                "USER",
                "ST",
                "TIME",
                "NODES",
                "NODELIST(REASON)"
            ]),
            slurm_row(&[
                "4201",
                "gpu",
                "train",
                "research",
                "R",
                "1:02:03",
                "2",
                "gpu[01-02]"
            ]),
            slurm_row(&[
                "4202",
                "gpu",
                "next",
                "research",
                "PD",
                "0:00",
                "1",
                "(Resources)"
            ])
        )
    }
    fn info(owner: &str, uid: &str, id: &str, fields: &str) -> String {
        format!("JobId={id} JobName=training {fields}\n   UserId={owner}({uid}) GroupId=research(1001)\n   Priority=12000 Account=research QOS=normal\n   JobState=RUNNING Reason=None\n")
    }
    fn summary(processes: bool, truncated: bool) -> String {
        let mut value =
            String::from("| NVIDIA-SMI 580.95.05 Driver Version: 580.95.05 CUDA Version: 13.0 |\n");
        for index in 0..8 {
            value+=&format!("| {index}  NVIDIA GeForce RTX 4090       Off |   00000000:18:00.0 Off | Off |\n|  0% 32C P8 26W / 450W | 19MiB / 24564MiB | 0% Default |\n| | | N/A |\n+---+---+---+\n");
        }
        if processes {
            value+="| Processes: |\n| GPU GI CI PID Type Process name GPU Memory |\n|============================|\n";
            for index in 0..8 {
                value += &format!(
                    "| {index} N/A N/A {} G /usr/lib/xorg/Xorg 4MiB |\n",
                    1000 + index
                );
            }
            value+="| 0 N/A N/A 2000 C python 15MiB |\n| 1 N/A N/A 2001 C+G python 15MiB |\n| 2 N/A N/A 2002 M nvidia-cuda-mps-server 15MiB |\n";
            if !truncated {
                value += "+----------------------------+\n";
            }
        }
        value
    }
    #[test]
    fn csv_quoted_fields_multiline_and_incomplete_output() {
        assert_eq!(
            parse_csv("\u{feff}0,\"a,b\",\"x\"\"y\"\r\n1,\"multi\nline\", z\n").unwrap(),
            vec![vec!["0", "a,b", "x\"y"], vec!["1", "multi\nline", "z"]]
        );
        assert!(parse_csv("0,\"half").is_err());
        assert!(parse_gpu_rows("0, GPU-a, name, 10, 0\n").is_err());
        assert!(parse_gpu_rows("").is_err());
        assert!(parse_compute_rows("GPU-a, wrong,python,10").is_err());
    }
    #[test]
    fn gpu_stability_and_compute_and_unknown_measurements_fail_closed() {
        let rows =
            parse_gpu_rows("1, GPU-b, RTX 4090, 24564, 19, 0\n0, GPU-a, RTX 4090, 24564, 0, 0\n")
                .unwrap();
        let first = summarize_gpu(&rows, &[], &options(), &[]);
        assert_eq!(first["cards"][0]["uuid"], "GPU-a");
        assert_eq!(first["idleCount"], 0);
        let second = summarize_gpu(&rows, &[], &options(), first["cards"].as_array().unwrap());
        assert_eq!(second["idleCount"], 2);
        let compute = parse_compute_rows("GPU-a,123,python,0\n").unwrap();
        let busy = summarize_gpu(
            &rows,
            &compute,
            &options(),
            second["cards"].as_array().unwrap(),
        );
        assert_eq!(busy["idleCount"], 1);
        let mig = parse_compute_rows("MIG-unmapped,123,python,0\n").unwrap();
        assert_eq!(
            summarize_gpu(&rows, &mig, &options(), &[])["cards"][1]["idleCandidate"],
            false
        );
        let unknown = parse_gpu_rows("0,GPU-a,RTX,N/A,N/A,N/A").unwrap();
        assert_eq!(
            summarize_gpu(&unknown, &[], &json!({"stableSamples":1}), &[])["idleCount"],
            0
        );
    }
    #[test]
    fn summary_display_processes_allow_idle_compute_and_mps_do_not() {
        let (rows, processes, complete) = parse_summary(&summary(true, false)).unwrap();
        assert!(complete);
        assert_eq!(rows.len(), 8);
        assert_eq!(processes.len(), 3);
        let value = summarize_gpu(&rows, &processes, &json!({"stableSamples":1}), &[]);
        assert_eq!(value["idleCount"], 5);
        for malformed in [
            summary(false, false),
            summary(true, true),
            summary(true, false).replace("2002 M", "2002 UNKNOWN"),
        ] {
            let (rows, processes, complete) = parse_summary(&malformed).unwrap();
            assert!(!complete);
            assert_eq!(
                summarize_gpu(&rows, &processes, &json!({"stableSamples":1}), &[])["idleCount"],
                0
            );
        }
        assert!(parse_summary("not a summary").is_err());
    }
    #[test]
    fn capability_absence_requires_an_explicit_device_diagnostic() {
        assert_eq!(
            gpu_availability(&QueryError::new("No devices were found")),
            "absent"
        );
        assert_eq!(
            gpu_availability(&QueryError::new("bash: nvidia-smi: command not found")),
            "unavailable"
        );
        for message in [
            "Failed to initialize NVML",
            "couldn't communicate with the NVIDIA driver",
            "permission denied",
            "query timeout",
        ] {
            assert_eq!(gpu_availability(&QueryError::new(message)), "unknown");
        }
        assert_eq!(
            slurm_availability(&QueryError::new(
                "squeue: Unable to contact slurm controller"
            )),
            "available"
        );
        assert_eq!(
            slurm_availability(&QueryError::new("squeue: command not found")),
            "unavailable"
        );
    }
    #[test]
    fn slurm_parses_default_queue_and_preserves_full_names_and_federation() {
        let jobs = parse_squeue(&queue()).unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0]["stateLabel"], "运行中");
        assert_eq!(jobs[1]["reasonLabel"], "等待可用资源");
        let long=queue()+"4203 gpu long training name research_user_long PD 0:00 1 (ReqNodeNotAvail, UnavailableNodes:gpu[01,03])\n";
        let jobs = parse_squeue(&long).unwrap();
        assert_eq!(jobs[2]["name"], "long training name");
        assert_eq!(jobs[2]["user"], "research_user_long");
        assert_eq!(
            jobs[2]["reasonLabel"],
            "指定节点当前不可用；不可用节点：gpu[01,03]"
        );
        let cluster = parse_squeue(&format!("CLUSTER: remote\n{}", queue())).unwrap();
        assert_eq!(cluster[0]["cluster"], "remote");
        assert!(parse_squeue("JOBID PARTITION OTHER HEADER").is_err());
        assert!(parse_squeue("squeue: controller unavailable").is_err());
        assert!(parse_squeue(&(queue() + "bad row\n")).is_err());
    }
    #[test]
    fn slurm_translates_state_and_dynamic_reason_families() {
        assert_eq!(translate_state("OUT_OF_MEMORY+"), ("内存不足", "negative"));
        assert_eq!(translate_state("NEW_STATE"), ("未知状态", "neutral"));
        assert_eq!(translate_reason("AssocGrpCpuLimit,QOSMaxMemoryPerUser,MaxJobsPerAccount"),"关联配额的处理器总量已达上限；服务质量配额的单用户内存请求超过上限；超过服务质量配额的单账户运行作业数上限");
        assert_eq!(
            translate_reason("Dependency:afterok:4201,NewReason"),
            "等待依赖作业满足条件：afterok:4201；未知原因"
        );
        assert_eq!(
            squeue_command("tester", "mine").unwrap(),
            "squeue -u tester"
        );
        assert_eq!(
            squeue_command("a; id", "mine").unwrap(),
            "squeue -u 'a; id'"
        );
        assert!(squeue_command("test\nid", "mine").is_err());
    }
    #[test]
    fn job_ids_are_bounded_and_cannot_add_shell_commands_or_arguments() {
        let id = validate_job_id("4201_[0-8:2,11,15-19%3]").unwrap();
        assert_eq!(id.kind, "array-group");
        assert_eq!(id.command_id, "4201_[0-8:2,11,15-19]");
        assert_eq!(id.info_command(), "scontrol show job 4201");
        assert_eq!(literal(&id.command_id), "'4201_[0-8:2,11,15-19]'");
        for id in [
            "",
            "0",
            "-1",
            "--user=other",
            "4201; id",
            "4201\nid",
            "$(id)",
            "4201.batch",
            "4201,4202",
            "4201_[9-1]",
            "4201_[1-9:0]",
            "4201_[1-9%0]",
            "4201_[1]'",
            "4201_1+",
        ] {
            assert!(validate_job_id(id).is_err(), "{id}");
        }
    }
    #[test]
    fn ownership_uses_authoritative_userid_and_exact_job_family() {
        let output = info("research_user_long", "1001", "4201", "");
        assert!(assert_owned_info(
            &output,
            &validate_job_id("4201").unwrap(),
            "research_user_long"
        )
        .is_ok());
        assert!(assert_owned_info(&output, &validate_job_id("4201").unwrap(), "research").is_err());
        assert!(assert_owned_info(
            &output,
            &validate_job_id("4202").unwrap(),
            "research_user_long"
        )
        .is_err());
        let array = info(
            "research_user_long",
            "1001",
            "4202",
            "ArrayJobId=4201 ArrayTaskId=2",
        );
        assert!(assert_owned_info(
            &array,
            &validate_job_id("4201_2").unwrap(),
            "research_user_long"
        )
        .is_ok());
        assert!(assert_owned_info(
            &array,
            &validate_job_id("4201_3").unwrap(),
            "research_user_long"
        )
        .is_err());
        assert!(assert_owned_info(
            &array,
            &validate_job_id("4201_[1-3%2]").unwrap(),
            "research_user_long"
        )
        .is_ok());
        let het = info(
            "research_user_long",
            "1001",
            "4210",
            "HetJobId=4201 HetJobOffset=1",
        );
        assert!(assert_owned_info(
            &het,
            &validate_job_id("4201+1").unwrap(),
            "research_user_long"
        )
        .is_ok());
        assert!(assert_owned_info(
            &het,
            &validate_job_id("4201+0").unwrap(),
            "research_user_long"
        )
        .is_err());
        assert!(parse_job_info(&(output.clone() + " UserId=other(1002)\n")).is_err());
        assert!(parse_job_info(&info(
            "research_user_long",
            "1001",
            "4201",
            "ArrayJobId=4201 ArrayJobId=4301"
        ))
        .is_err());
    }
    #[test]
    fn priorities_are_numeric_safe_integers_only() {
        assert_eq!(
            parse_priorities("Priority=0\nPriority=4294967295\n").unwrap(),
            vec![0, 4294967295]
        );
        for value in [
            "",
            "Priority=unknown",
            "SomePriority=42",
            "Priority=4 trailing",
            "Priority=-1",
            "Priority=9007199254740992",
        ] {
            assert!(parse_priorities(value).is_err());
        }
    }
    #[test]
    fn storage_aggregates_allocated_bytes_and_partial_results_strictly() {
        let state = parse_storage_scan(
            "LION_STORAGE_V1\t1001\nHOME\t4096\t2\t1\t0\nTMP\t8192\t3\t2\t0\n",
            "tester",
        )
        .unwrap();
        assert_eq!(state["homePath"], "/home/tester");
        assert_eq!(state["totalBytes"], 12288);
        assert_eq!(state["partial"], false);
        let state = parse_storage_scan(
            "LION_STORAGE_V1\t1001\nHOME\t0\t0\t0\t1\nTMP\t8192\t3\t2\t0\n",
            "tester",
        )
        .unwrap();
        assert!(state["homeBytes"].is_null());
        assert!(state["totalBytes"].is_null());
        assert_eq!(state["partial"], true);
        for output in [
            "LION_STORAGE_V1\t1001\nHOME\t0\t0\t0\t127\nTMP\t0\t0\t0\t0\n",
            "LION_STORAGE_V1\t1001\nHOME\t0\t1\t2\t0\nTMP\t0\t0\t0\t0\n",
            "LION_STORAGE_V1\t1001\nHOME\t9007199254740992\t2\t1\t0\nTMP\t0\t0\t0\t0\n",
        ] {
            assert!(parse_storage_scan(output, "tester").is_err());
        }
        assert!(home_path("../tester").is_err());
        assert!(home_path("a\\b").is_err());
        assert!(build_tmp_cleanup(0).is_err());
    }
    #[test]
    fn storage_shell_restricts_traversal_ownership_and_deletion() {
        let scan = build_storage_scan("test'er; id").unwrap();
        assert!(scan.contains("'/home/test'\"'\"'er; id'"));
        assert!(scan.contains("find -H"));
        assert!(scan.contains("find -P '/tmp' -xdev -mindepth 1"));
        assert!(scan.contains("seen[$2 \":\" $3]"));
        assert!(scan.contains("$4 * 512"));
        let cleanup = build_tmp_cleanup(1001).unwrap();
        assert!(cleanup.contains("PATH=/usr/bin:/bin"));
        assert!(cleanup.contains("-execdir /bin/sh -c"));
        assert!(cleanup.contains("-maxdepth 0 -uid"));
        assert!(cleanup.contains("-type d -empty"));
        assert!(cleanup.contains("[ -L /tmp ]"));
        assert!(cleanup.contains("! -readable -o ! -executable"));
        assert!(cleanup.contains("[ \"$uid\" != '1001' ]"));
        assert!(!cleanup.contains("rm -rf"));
    }

    #[test]
    #[cfg(any(windows, target_os = "linux"))]
    fn real_gnu_find_fixture_preserves_targets_and_deduplicates_allocated_blocks() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let bash = if cfg!(windows) {
            r"C:\Program Files\Git\bin\bash.exe"
        } else {
            "/bin/bash"
        };
        if !std::path::Path::new(bash).is_file() {
            return;
        }
        fn shell(bash: &str, script: &str) -> std::process::Output {
            let mut child = Command::new(bash)
                .args(["--noprofile", "--norc", "-s"])
                .env("MSYS", "")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(script.as_bytes())
                .unwrap();
            child.wait_with_output().unwrap()
        }
        let temp_base = std::env::temp_dir().canonicalize().unwrap();
        let fixture = temp_base.join(format!("lion-ssh-storage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&fixture).unwrap();
        struct Fixture(std::path::PathBuf, std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                // This exact generated directory is the only recursive cleanup
                // target. remove_dir_all unlinks symlinks without following them.
                if let Ok(resolved) = self.0.canonicalize() {
                    assert_eq!(resolved.parent(), Some(self.1.as_path()));
                    assert!(resolved
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("lion-ssh-storage-"));
                    std::fs::remove_dir_all(resolved).unwrap();
                }
            }
        }
        let _fixture_guard = Fixture(fixture.clone(), temp_base);
        let home = fixture.join("home/tester");
        let tmp = fixture.join("tmp");
        let outside = fixture.join("outside");
        for directory in [&home, &tmp, &outside] {
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::create_dir(tmp.join("nested")).unwrap();
        std::fs::write(home.join("home-data"), vec![b'H'; 8192]).unwrap();
        std::fs::write(outside.join("marker"), vec![b'X'; 4096]).unwrap();
        std::fs::write(tmp.join("space and $brackets[1]"), vec![b'T'; 8192]).unwrap();
        std::fs::write(tmp.join("nested/own-data"), b"temporary").unwrap();
        std::fs::hard_link(outside.join("marker"), tmp.join("hardlink")).unwrap();
        std::fs::hard_link(outside.join("marker"), tmp.join("hardlink-two")).unwrap();
        let slash = |path: &std::path::Path| {
            path.to_string_lossy()
                .replace('\\', "/")
                .trim_start_matches("//?/")
                .to_owned()
        };
        let shell_tmp = slash(&tmp);
        let shell_home = slash(&home);
        let shell_outside = slash(&outside);
        // MSYS ln may copy a directory when native symlink privileges are
        // disabled. A Windows junction requires no symlink privilege and GNU
        // find correctly sees it as a directory link under -P.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let junction = Command::new("cmd.exe")
                .args(["/C", "mklink", "/J"])
                .arg(tmp.join("directory-link"))
                .arg(&outside)
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(
                junction.status.success(),
                "{}",
                String::from_utf8_lossy(&junction.stderr)
            );
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(&outside, tmp.join("directory-link")).unwrap();
        }
        let links = shell(
            bash,
            &format!(
                "ln -s {} {}\ntest -L {}\n",
                quote(&format!("{shell_outside}/marker")),
                quote(&format!("{shell_tmp}/file-link")),
                quote(&format!("{shell_tmp}/directory-link"))
            ),
        );
        assert!(
            links.status.success(),
            "{}",
            String::from_utf8_lossy(&links.stderr)
        );
        let uid_result = shell(bash, "id -u\n");
        let uid = String::from_utf8(uid_result.stdout)
            .unwrap()
            .trim()
            .parse::<u64>()
            .unwrap();
        if uid == 0 {
            return;
        } // Cleanup deliberately refuses root on Linux too.
        let isolated = |script: String| {
            // On Linux the generated home path itself starts with /tmp, so
            // replace the original /tmp references before inserting that path.
            script
                .replace("/tmp", &shell_tmp)
                .replace("/home/tester", &shell_home)
        };
        let scan = shell(bash, &isolated(build_storage_scan("tester").unwrap()));
        assert!(
            scan.status.success(),
            "{}",
            String::from_utf8_lossy(&scan.stderr)
        );
        let scan_stdout = String::from_utf8(scan.stdout).unwrap();
        let scan_stderr = String::from_utf8_lossy(&scan.stderr);
        let state = parse_storage_scan(&scan_stdout, "tester").unwrap_or_else(|error| {
            panic!("{error}; scan stdout: {scan_stdout:?}; scan stderr: {scan_stderr:?}")
        });
        assert_eq!(
            state["partial"], false,
            "scan stdout: {scan_stdout:?}; scan stderr: {scan_stderr:?}"
        );
        let preview = shell(bash, &isolated(build_tmp_preview()));
        let preview = parse_tmp_preview(&String::from_utf8(preview.stdout).unwrap()).unwrap();
        assert_eq!(preview["fileCount"], 6); // Two hardlinks, two symlinks, and two ordinary files.
        let stat = shell(
            bash,
            &format!(
                "stat -c '%d:%i %b %B' {} {} {} {} {} {} {}\n",
                quote(&format!("{shell_tmp}/space and $brackets[1]")),
                quote(&format!("{shell_tmp}/hardlink")),
                quote(&format!("{shell_tmp}/hardlink-two")),
                quote(&format!("{shell_tmp}/nested")),
                quote(&format!("{shell_tmp}/nested/own-data")),
                quote(&format!("{shell_tmp}/file-link")),
                quote(&format!("{shell_tmp}/directory-link"))
            ),
        );
        assert!(stat.status.success());
        let mut seen = HashSet::new();
        let mut expected_bytes = 0u64;
        for line in String::from_utf8(stat.stdout).unwrap().lines() {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if seen.insert(fields[0].to_owned()) {
                // MSYS stat reports %b in 1024-byte units, whereas GNU find
                // keeps its documented 512-byte %b units on this platform.
                expected_bytes +=
                    fields[1].parse::<u64>().unwrap() * fields[2].parse::<u64>().unwrap();
            }
        }
        assert_eq!(state["tmpBytes"], expected_bytes);
        let bad_uid = shell(bash, &isolated(build_tmp_cleanup(uid + 1).unwrap()));
        assert!(!bad_uid.status.success());
        assert!(tmp.join("hardlink").is_file());
        let cleaned = shell(bash, &isolated(build_tmp_cleanup(uid).unwrap()));
        assert!(
            cleaned.status.success(),
            "{}",
            String::from_utf8_lossy(&cleaned.stderr)
        );
        let cleaned = parse_tmp_cleanup(&String::from_utf8(cleaned.stdout).unwrap()).unwrap();
        assert_eq!(cleaned["partial"], false);
        assert!(cleaned["deletedCount"].as_u64().unwrap() >= 7);
        assert_eq!(std::fs::read_dir(&tmp).unwrap().count(), 0);
        assert_eq!(
            std::fs::read(outside.join("marker")).unwrap(),
            vec![b'X'; 4096]
        );
        assert_eq!(
            std::fs::read(home.join("home-data")).unwrap(),
            vec![b'H'; 8192]
        );
    }

    struct Fake {
        commands: Mutex<Vec<String>>,
        owner: Mutex<String>,
        uid: Mutex<String>,
        fail_cancel: AtomicBool,
    }
    impl Fake {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                commands: Mutex::new(Vec::new()),
                owner: Mutex::new("research_user_long".into()),
                uid: Mutex::new("1001".into()),
                fail_cancel: AtomicBool::new(false),
            })
        }
    }
    #[async_trait::async_trait]
    impl Executor for Fake {
        async fn exec(
            &self,
            command: &str,
            _timeout: Duration,
            _max_output: usize,
        ) -> Result<CommandOutput, String> {
            self.commands.lock().await.push(command.into());
            let stdout = if command.starts_with("scontrol") {
                if command.contains("grep -o") {
                    "Priority=12000\n".into()
                } else {
                    info(
                        &*self.owner.lock().await,
                        &*self.uid.lock().await,
                        "4201",
                        "",
                    )
                }
            } else if command == "scancel 4201" {
                if self.fail_cancel.load(Ordering::SeqCst) {
                    return Err("connection closed before acknowledgement".into());
                }
                String::new()
            } else if command.starts_with("squeue") {
                queue()
            } else if command == GPU_QUERY {
                "0,GPU-a,RTX 4090,24564,19,0\n".into()
            } else if command == COMPUTE_QUERY {
                String::new()
            } else if command.starts_with("# Lion tmp preview") {
                "LION_TMP_PREVIEW_V1\t1001\nTMP\t4096\t2\t1\t0\n".into()
            } else if command.starts_with("# Lion tmp cleanup") {
                "LION_TMP_CLEANUP_V1\t1001\nRESULT\t2\t0\n".into()
            } else if command.starts_with("# Lion storage scan") {
                "LION_STORAGE_V1\t1001\nHOME\t4096\t2\t1\t0\nTMP\t8192\t3\t2\t0\n".into()
            } else {
                return Err(format!("Unexpected command: {command}"));
            };
            Ok(CommandOutput {
                stdout,
                stderr: String::new(),
                exit_code: Some(0),
            })
        }
    }
    async fn harness() -> (Arc<Monitor>, Arc<Fake>) {
        let executor = Fake::new();
        let monitor = Monitor::new(
            json!({"id":"fixture","username":"research_user_long","gpu":{}}),
            executor.clone(),
            Arc::new(|_, _| {}),
        );
        monitor.refresh_slurm().await;
        (monitor, executor)
    }
    #[tokio::test]
    async fn initializing_scope_never_waits_for_remote_queries() {
        let executor = Fake::new();
        let monitor = Monitor::new(
            json!({"id":"fixture","username":"research_user_long","gpu":{}}),
            executor.clone(),
            Arc::new(|_, _| {}),
        );
        monitor.initialize_slurm_scope("all").await.unwrap();
        assert_eq!(monitor.slurm.lock().await["scope"], "all");
        assert!(executor.commands.lock().await.is_empty());
        assert!(monitor.initialize_slurm_scope("invalid").await.is_err());
        monitor.refresh_slurm().await;
        assert_eq!(executor.commands.lock().await.as_slice(), &["squeue"]);
    }
    #[tokio::test]
    async fn cancellation_requires_one_use_confirmation_and_rechecks_uid() {
        let (monitor, executor) = harness().await;
        let preview = monitor
            .action("prepare-job-cancel", &[json!("4201")])
            .await
            .unwrap();
        assert_eq!(preview["owner"], "research_user_long");
        monitor
            .action("cancel-job", &[json!("4201"), preview["token"].clone()])
            .await
            .unwrap();
        assert!(monitor
            .action("cancel-job", &[json!("4201"), preview["token"].clone()])
            .await
            .is_err());
        assert_eq!(
            executor
                .commands
                .lock()
                .await
                .iter()
                .filter(|v| *v == "scancel 4201")
                .count(),
            1
        );
        let preview = monitor
            .action("prepare-job-cancel", &[json!("4201")])
            .await
            .unwrap();
        *executor.uid.lock().await = "2001".into();
        assert!(monitor
            .action("cancel-job", &[json!("4201"), preview["token"].clone()])
            .await
            .is_err());
        assert_eq!(
            executor
                .commands
                .lock()
                .await
                .iter()
                .filter(|v| *v == "scancel 4201")
                .count(),
            1
        );
    }
    #[tokio::test]
    async fn cancellation_failure_never_retries_or_reuses_token() {
        let (monitor, executor) = harness().await;
        executor.fail_cancel.store(true, Ordering::SeqCst);
        let preview = monitor
            .action("prepare-job-cancel", &[json!("4201")])
            .await
            .unwrap();
        for _ in 0..2 {
            assert!(monitor
                .action("cancel-job", &[json!("4201"), preview["token"].clone()])
                .await
                .is_err());
        }
        assert_eq!(
            executor
                .commands
                .lock()
                .await
                .iter()
                .filter(|v| *v == "scancel 4201")
                .count(),
            1
        );
    }
    #[tokio::test]
    async fn stale_foreign_and_stopped_connections_cannot_cancel() {
        let (monitor, executor) = harness().await;
        assert!(monitor
            .action("prepare-job-cancel", &[json!("9999")])
            .await
            .is_err());
        monitor.slurm.lock().await["lastUpdated"] = json!(now().saturating_sub(90_001));
        assert!(monitor
            .action("prepare-job-cancel", &[json!("4201")])
            .await
            .is_err());
        monitor.refresh_slurm().await;
        monitor.slurm.lock().await["jobs"][0]["cluster"] = json!("foreign");
        assert!(monitor
            .action("prepare-job-cancel", &[json!("4201")])
            .await
            .is_err());
        monitor.refresh_slurm().await;
        let preview = monitor
            .action("prepare-job-cancel", &[json!("4201")])
            .await
            .unwrap();
        monitor.stop().await;
        assert!(monitor
            .action("cancel-job", &[json!("4201"), preview["token"].clone()])
            .await
            .is_err());
        assert!(!executor
            .commands
            .lock()
            .await
            .iter()
            .any(|v| v.starts_with("scancel")));
    }
    #[tokio::test]
    async fn tmp_cleanup_has_separate_expiring_one_use_confirmation() {
        let (monitor, executor) = harness().await;
        let preview = monitor.action("prepare-tmp-cleanup", &[]).await.unwrap();
        assert_eq!(preview["uid"], 1001);
        assert!(monitor
            .action("cancel-job", &[json!("4201"), preview["token"].clone()])
            .await
            .is_err());
        let preview = monitor.action("prepare-tmp-cleanup", &[]).await.unwrap();
        let result = monitor
            .action("cleanup-tmp", &[preview["token"].clone()])
            .await
            .unwrap();
        assert_eq!(result["deletedCount"], 2);
        assert!(monitor
            .action("cleanup-tmp", &[preview["token"].clone()])
            .await
            .is_err());
        assert_eq!(
            executor
                .commands
                .lock()
                .await
                .iter()
                .filter(|v| v.starts_with("# Lion tmp cleanup"))
                .count(),
            1
        );
        let preview = monitor.action("prepare-tmp-cleanup", &[]).await.unwrap();
        monitor
            .tokens
            .lock()
            .await
            .get_mut(preview["token"].as_str().unwrap())
            .unwrap()
            .expires = now() - 1;
        assert!(monitor
            .action("cleanup-tmp", &[preview["token"].clone()])
            .await
            .is_err());
    }
}
