//! JSON compatibility with the original desktop backend and renderer.
use serde_json::{json, Value};
use std::collections::HashSet;

fn js_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Array(values) => values.iter().map(js_string).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

fn text(value: &Value, max: usize, label: &str, required: bool) -> Result<String, String> {
    let cleaned = js_string(value)
        .chars()
        .filter(|c| !matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .collect::<String>();
    let result = cleaned.trim();
    if (required && result.is_empty())
        || result.encode_utf16().count() > max
        || result.chars().any(|c| c <= '\u{001f}' || c == '\u{007f}')
    {
        return Err(format!("{label}格式不正确"));
    }
    Ok(result.to_owned())
}

fn number(value: &Value, fallback: u64, min: u64, max: u64, label: &str) -> Result<u64, String> {
    let number = match value {
        Value::Null => Some(fallback as f64),
        Value::Number(value) => value.as_f64(),
        Value::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
        Value::String(value) => {
            let value = value.trim();
            if value.is_empty() {
                Some(0.0)
            } else {
                value.parse::<f64>().ok()
            }
        }
        _ => None,
    };
    match number {
        Some(number)
            if number.is_finite()
                && number.fract() == 0.0
                && number >= min as f64
                && number <= max as f64 =>
        {
            Ok(number as u64)
        }
        _ => Err(format!("{label}应在 {min}–{max} 之间")),
    }
}

pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|n| n != 0.0),
        Value::String(value) => !value.is_empty(),
        _ => true,
    }
}

fn fallback(value: &Value, default: &str) -> Value {
    if truthy(value) {
        value.clone()
    } else {
        Value::String(default.into())
    }
}

pub fn gpu_settings(input: &Value) -> Result<Value, String> {
    Ok(json!({
        "enabled": input["enabled"] != false,
        "intervalSec": number(&input["intervalSec"], 5, 2, 300, "检测间隔")?,
        "idleUtil": number(&input["idleUtil"], 5, 0, 100, "空闲利用率阈值")?,
        "idleMemoryMiB": number(&input["idleMemoryMiB"], 512, 0, 1_048_576, "空闲显存阈值")?,
        "stableSamples": number(&input["stableSamples"], 2, 1, 10, "连续采样次数")?
    }))
}

fn normalize_tunnel(input: &Value) -> Result<Value, String> {
    let direction = input["direction"].as_str().unwrap_or("");
    if !["local", "remote"].contains(&direction) {
        return Err("请选择正向或反向隧道".into());
    }
    Ok(json!({
        "id": text(&fallback(&input["id"], &uuid::Uuid::new_v4().to_string()), 100, "隧道标识", true)?,
        "name": text(&fallback(&input["name"], if direction == "local" { "正向隧道" } else { "反向隧道" }), 80, "隧道名称", true)?,
        "direction": direction,
        "bindHost": text(&fallback(&input["bindHost"], "127.0.0.1"), 253, "监听地址", true)?,
        "bindPort": number(&input["bindPort"], 6006, 1, 65535, "监听端口")?,
        "targetHost": text(&fallback(&input["targetHost"], "127.0.0.1"), 253, "目标地址", true)?,
        "targetPort": number(&input["targetPort"], 6006, 1, 65535, "目标端口")?,
        "enabled": input["enabled"] != false,
        "healthIntervalSec": number(&input["healthIntervalSec"], 15, 5, 300, "隧道检测间隔")?
    }))
}

pub fn normalize_profile(input: &Value) -> Result<Value, String> {
    if !input.is_object() {
        return Err("设备配置不能为空".into());
    }
    let auth = input["authType"].as_str().unwrap_or("");
    if !["key", "password"].contains(&auth) {
        return Err("认证方式不正确".into());
    }
    let raw_tunnels = match input.get("tunnels") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(values)) => values.as_slice(),
        _ => return Err("隧道配置格式不正确".into()),
    };
    if raw_tunnels.len() > 100 {
        return Err("每台设备最多支持 100 条隧道".into());
    }
    let tunnels = raw_tunnels
        .iter()
        .map(normalize_tunnel)
        .collect::<Result<Vec<_>, _>>()?;
    let mut ids = HashSet::new();
    let mut endpoints = HashSet::new();
    for tunnel in &tunnels {
        if !ids.insert(tunnel["id"].as_str().unwrap()) {
            return Err("隧道标识重复".into());
        }
        if tunnel["enabled"] == true {
            let endpoint = format!(
                "{}|{}|{}",
                tunnel["direction"], tunnel["bindHost"], tunnel["bindPort"]
            );
            if !endpoints.insert(endpoint) {
                return Err(format!("监听端口 {} 在当前设备中重复", tunnel["bindPort"]));
            }
        }
    }
    Ok(json!({
        "id": text(&fallback(&input["id"], &uuid::Uuid::new_v4().to_string()), 100, "设备标识", true)?,
        "name": text(&input["name"], 80, "设备名称", true)?,
        "host": text(&input["host"], 253, "服务器地址", true)?,
        "port": number(&input["port"], 22, 1, 65535, "SSH 端口")?,
        "username": text(&input["username"], 80, "用户名", true)?,
        "authType": auth,
        "keyPath": text(&input["keyPath"], 2048, "密钥路径", auth == "key")?,
        "autoReconnect": input["autoReconnect"] != false,
        "gpu": gpu_settings(&input["gpu"])?,
        "tunnels": tunnels
    }))
}

pub fn blank_state(profile: &Value) -> Value {
    let username = profile["username"].as_str().unwrap_or("");
    let home_path = if username.is_empty()
        || username.encode_utf16().count() > 80
        || [".", ".."].contains(&username)
        || username
            .chars()
            .any(|c| matches!(c, '/' | '\\') || c <= '\u{001f}' || c == '\u{007f}')
    {
        String::new()
    } else {
        format!("/home/{username}")
    };
    let tunnels = profile["tunnels"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|t| json!({"id": t["id"], "status": "stopped"}))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "profileId": profile["id"], "status": "disconnected",
        "gpu": { "status": "unknown", "availability": "unknown", "availabilityReason": null, "availabilityCheckedAt": null,
            "cards": [], "idleCount": 0, "total": 0, "lastUpdated": null, "nextPollAt": null, "intervalSec": profile["gpu"]["intervalSec"] },
        "slurm": { "availability": "unknown", "status": "unknown", "scope": "mine", "jobs": [], "lastUpdated": null,
            "nextPollAt": null, "intervalSec": 30, "error": null, "availabilityReason": null },
        "storage": { "status": "unknown", "homePath": home_path, "homeBytes": null, "tmpBytes": null, "totalBytes": null,
            "lastUpdated": null, "nextPollAt": null, "intervalSec": 300, "error": null, "warnings": [], "partial": false },
        "tunnels": tunnels, "events": [], "codex": crate::codex::blank_state()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile() -> Value {
        json!({"id":"saved", "name":"服务器", "host":"192.0.2.10", "username":"user", "authType":"password", "tunnels":[]})
    }
    #[test]
    fn normalizes_legacy_profile_without_changing_json_contract() {
        assert_eq!(
            normalize_profile(&profile()).unwrap(),
            json!({
                "id":"saved", "name":"服务器", "host":"192.0.2.10", "port":22, "username":"user", "authType":"password", "keyPath":"", "autoReconnect":true,
                "gpu":{"enabled":true, "intervalSec":5, "idleUtil":5, "idleMemoryMiB":512, "stableSamples":2}, "tunnels":[]
            })
        );
    }
    #[test]
    fn detects_enabled_endpoint_collisions_and_duplicate_ids() {
        let mut value = profile();
        value["tunnels"] =
            json!([{"id":"a", "direction":"local"}, {"id":"b", "direction":"local"}]);
        assert!(normalize_profile(&value)
            .unwrap_err()
            .contains("监听端口 6006"));
        value["tunnels"][1]["enabled"] = json!(false);
        assert!(normalize_profile(&value).is_ok());
        value["tunnels"][1]["id"] = json!("a");
        assert!(normalize_profile(&value).unwrap_err().contains("标识重复"));
    }
    #[test]
    fn strips_bidi_and_checks_utf16_lengths_and_integer_bounds() {
        let mut value = profile();
        value["name"] = json!(" \u{202e}服务器\u{2069} ");
        value["port"] = json!("2222");
        let normalized = normalize_profile(&value).unwrap();
        assert_eq!(normalized["name"], "服务器");
        assert_eq!(normalized["port"], 2222);
        value["port"] = json!(22.1);
        assert!(normalize_profile(&value).is_err());
        value["port"] = json!(22);
        value["name"] = json!("😀".repeat(41));
        assert!(normalize_profile(&value).is_err());
        assert!(gpu_settings(&json!({"intervalSec":1})).is_err());
    }
    #[test]
    fn initial_state_retains_monitor_defaults() {
        let state = blank_state(&normalize_profile(&profile()).unwrap());
        assert_eq!(state["gpu"]["intervalSec"], 5);
        assert_eq!(state["slurm"]["intervalSec"], 30);
        assert_eq!(state["storage"]["intervalSec"], 300);
        assert_eq!(state["storage"]["homePath"], "/home/user");
        assert_eq!(state["status"], "disconnected");
        assert_eq!(state["events"], json!([]));
    }
}
