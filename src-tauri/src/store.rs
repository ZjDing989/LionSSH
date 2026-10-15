//! Existing `data` files stay compatible, including native:v1 Windows DPAPI credentials.
use crate::{crypto, model};
use base64::{
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
    Engine,
};
use hmac::{Hmac, Mac};
use serde_json::{json, Map, Value};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

const FILES: [&str; 4] = [
    "profiles.json",
    "credentials.json",
    "known-hosts.json",
    "settings.json",
];
const SECRET_PREFIX: &str = "native:v1:";

pub struct Store {
    pub dir: PathBuf,
    pub profiles: Vec<Value>,
    pub settings: Value,
    secrets: Map<String, Value>,
    trusted: Map<String, Value>,
    // Keep future/extension fields on disk without changing the public profile contract.
    original_profiles: HashMap<String, Value>,
}

fn read_json(dir: &Path, name: &str, fallback: Value) -> Result<Value, String> {
    match fs::read(dir.join(name)) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|error| format!("无法读取配置 {name}：{error}"))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(fallback),
        Err(error) => Err(format!("无法读取配置 {name}：{error}")),
    }
}

fn object(value: Value, name: &str) -> Result<Map<String, Value>, String> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| format!("无法读取配置 {name}：必须是 JSON 对象"))
}

fn keep_unknown(original: &Value, normalized: &Value) -> Value {
    match (original, normalized) {
        (Value::Object(previous), Value::Object(current)) => {
            let mut merged = previous.clone();
            for (key, value) in current {
                merged.insert(
                    key.clone(),
                    previous
                        .get(key)
                        .map_or_else(|| value.clone(), |old| keep_unknown(old, value)),
                );
            }
            Value::Object(merged)
        }
        (Value::Array(previous), Value::Array(current)) => Value::Array(
            current
                .iter()
                .map(|value| {
                    previous
                        .iter()
                        .find(|old| old.get("id").is_some() && old.get("id") == value.get("id"))
                        .map_or_else(|| value.clone(), |old| keep_unknown(old, value))
                })
                .collect(),
        ),
        _ => normalized.clone(),
    }
}

impl Store {
    pub fn open(dir: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&dir).map_err(|error| format!("无法创建 data 目录：{error}"))?;
        let saved = read_json(&dir, FILES[0], Value::Null)?;
        let secrets = object(read_json(&dir, FILES[1], json!({}))?, FILES[1])?;
        let trusted = object(read_json(&dir, FILES[2], json!({}))?, FILES[2])?;
        let mut settings = Value::Object(object(read_json(&dir, FILES[3], json!({}))?, FILES[3])?);
        if !["light", "dark", "system"].contains(&settings["theme"].as_str().unwrap_or("")) {
            settings["theme"] = json!("system");
        }
        let mut profiles = Vec::new();
        let mut original_profiles = HashMap::new();
        if !saved.is_null() {
            let values = saved
                .as_array()
                .ok_or_else(|| "无法读取配置 profiles.json：必须是 JSON 数组".to_string())?;
            for value in values {
                let normalized = model::normalize_profile(value)
                    .map_err(|error| format!("无法读取配置 profiles.json：{error}"))?;
                original_profiles
                    .insert(normalized["id"].as_str().unwrap().to_owned(), value.clone());
                profiles.push(normalized);
            }
        }
        let mut store = Self {
            dir,
            profiles,
            settings,
            secrets,
            trusted,
            original_profiles,
        };
        if saved.is_null() {
            store.persist()?;
        }
        Ok(store)
    }

    pub fn public_profiles(&self) -> Vec<Value> {
        self.profiles
            .iter()
            .map(|profile| {
                let mut profile = profile.clone();
                profile["hasSavedSecret"] = json!(self
                    .secrets
                    .get(profile["id"].as_str().unwrap_or(""))
                    .is_some_and(model::truthy));
                profile
            })
            .collect()
    }

    pub fn get(&self, id: &str) -> Result<Value, String> {
        self.profiles
            .iter()
            .find(|profile| profile["id"] == id)
            .cloned()
            .ok_or_else(|| "设备不存在".into())
    }

    pub fn save(&mut self, input: &Value, secret: Option<&Value>) -> Result<Value, String> {
        let profile = model::normalize_profile(input)?;
        let id = profile["id"].as_str().unwrap().to_owned();
        let previous_profiles = self.profiles.clone();
        let previous_secrets = self.secrets.clone();
        // Encrypt first; a crypto failure must not mutate any in-memory setting.
        let secret_update = if let Some(remember) =
            secret.and_then(|secret| secret["remember"].as_bool())
        {
            if remember {
                let secret = secret.unwrap();
                let relevant = if profile["authType"] == "password" {
                    "password"
                } else {
                    "passphrase"
                };
                if secret
                    .get(relevant)
                    .is_some_and(|value| !value.is_null() && !value.is_string())
                {
                    return Err("凭据格式不正确".into());
                }
                let supplied = secret[relevant].as_str().filter(|value| !value.is_empty());
                let existing =
                    if supplied.is_none() && self.secrets.get(&id).is_some_and(model::truthy) {
                        self.read_secret(&id)?
                    } else {
                        json!({})
                    };
                let plain = supplied.or_else(|| {
                    existing[relevant]
                        .as_str()
                        .filter(|value| !value.is_empty())
                });
                if let Some(plain) = plain {
                    if plain.encode_utf16().count() > 4096 {
                        return Err("凭据格式不正确".into());
                    }
                    let mut data = Map::new();
                    data.insert(relevant.into(), json!(plain));
                    let encoded = crypto::transform("encrypt", &Value::Object(data).to_string())?;
                    if encoded.is_empty() {
                        return Err("系统加密未返回有效凭据".into());
                    }
                    Some(Some(json!(format!("{SECRET_PREFIX}{encoded}"))))
                } else {
                    Some(None)
                }
            } else {
                Some(None)
            }
        } else {
            None
        };
        if let Some(update) = secret_update {
            match update {
                Some(value) => {
                    self.secrets.insert(id.clone(), value);
                }
                None => {
                    self.secrets.remove(&id);
                }
            }
        }
        if let Some(index) = self.profiles.iter().position(|value| value["id"] == id) {
            self.profiles[index] = profile;
        } else {
            self.profiles.push(profile);
        }
        if let Err(error) = self.persist() {
            self.profiles = previous_profiles;
            self.secrets = previous_secrets;
            return Err(error);
        }
        self.public_profiles()
            .into_iter()
            .find(|value| value["id"] == id)
            .ok_or_else(|| "设备不存在".into())
    }

    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        self.get(id)?;
        let previous_profiles = self.profiles.clone();
        let previous_secrets = self.secrets.clone();
        self.profiles.retain(|profile| profile["id"] != id);
        self.secrets.remove(id);
        if let Err(error) = self.persist() {
            self.profiles = previous_profiles;
            self.secrets = previous_secrets;
            return Err(error);
        }
        self.original_profiles.remove(id);
        Ok(())
    }

    pub fn set_theme(&mut self, preference: &str) -> Result<(), String> {
        if !["light", "dark", "system"].contains(&preference) {
            return Err("请选择白天、夜晚或跟随系统".into());
        }
        let previous = self.settings.clone();
        self.settings["theme"] = json!(preference);
        if let Err(error) = self.persist() {
            self.settings = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn read_secret(&self, id: &str) -> Result<Value, String> {
        let Some(value) = self.secrets.get(id).filter(|value| model::truthy(value)) else {
            return Ok(json!({}));
        };
        let decoded = (|| {
            let encoded = value.as_str()?.strip_prefix(SECRET_PREFIX)?;
            let plaintext = crypto::transform("decrypt", encoded).ok()?;
            let data: Value = serde_json::from_str(&plaintext).ok()?;
            data.is_object().then_some(data)
        })();
        decoded.ok_or_else(|| "无法解密保存的凭据，请重新输入密码或密钥口令".into())
    }

    fn endpoint(profile: &Value) -> Result<String, String> {
        let host = profile["host"]
            .as_str()
            .ok_or_else(|| "服务器地址格式不正确".to_string())?;
        let port = profile["port"]
            .as_u64()
            .ok_or_else(|| "SSH 端口格式不正确".to_string())?;
        Ok(format!("{host}:{port}"))
    }

    pub fn trust(&mut self, profile: &Value, fingerprint: &str) -> Result<(), String> {
        let endpoint = Self::endpoint(profile)?;
        let previous = self.trusted.clone();
        self.trusted.insert(endpoint, json!([fingerprint]));
        if let Err(error) = self.persist() {
            self.trusted = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn fingerprints(&self, profile: &Value) -> Result<Vec<String>, String> {
        if let Some(pinned) = self
            .trusted
            .get(&Self::endpoint(profile)?)
            .and_then(Value::as_array)
            .filter(|array| !array.is_empty())
        {
            return Ok(pinned
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect());
        }
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
        let data = home.and_then(|home| {
            fs::read_to_string(PathBuf::from(home).join(".ssh").join("known_hosts")).ok()
        });
        Ok(data
            .map(|data| openssh_fingerprints(&data, profile))
            .unwrap_or_default())
    }

    pub fn persist(&mut self) -> Result<(), String> {
        let profiles = Value::Array(
            self.profiles
                .iter()
                .map(|profile| {
                    self.original_profiles
                        .get(profile["id"].as_str().unwrap_or(""))
                        .map_or_else(
                            || profile.clone(),
                            |original| keep_unknown(original, profile),
                        )
                })
                .collect(),
        );
        let values = [
            profiles,
            Value::Object(self.secrets.clone()),
            Value::Object(self.trusted.clone()),
            self.settings.clone(),
        ];
        // Stage every write before replacing any file. Keep snapshots to restore a partial commit.
        let mut staged = Vec::new();
        let mut previous = Vec::new();
        for (name, value) in FILES.iter().zip(values.iter()) {
            let file = self.dir.join(name);
            let old = match fs::read(&file) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => {
                    cleanup(&staged);
                    return Err(format!("无法保存配置 {name}：{error}"));
                }
            };
            let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
            match stage_file(&file, &bytes) {
                Ok(temp) => {
                    previous.push(old);
                    staged.push(temp);
                }
                Err(error) => {
                    cleanup(&staged);
                    return Err(error);
                }
            }
        }
        for (index, name) in FILES.iter().enumerate() {
            if let Err(error) = fs::rename(&staged[index], self.dir.join(name)) {
                let mut rollback_errors = Vec::new();
                for (restored, old) in previous.iter().enumerate().take(index) {
                    let target = self.dir.join(FILES[restored]);
                    let result = match old {
                        Some(bytes) => stage_file(&target, bytes).and_then(|temp| {
                            fs::rename(&temp, &target).map_err(|error| {
                                let _ = fs::remove_file(temp);
                                error.to_string()
                            })
                        }),
                        None => fs::remove_file(&target).map_err(|error| error.to_string()),
                    };
                    if let Err(error) = result {
                        rollback_errors.push(error);
                    }
                }
                cleanup(&staged);
                let suffix = if rollback_errors.is_empty() {
                    String::new()
                } else {
                    format!("；恢复原配置失败：{}", rollback_errors.join("；"))
                };
                return Err(format!("无法保存配置 {name}：{error}{suffix}"));
            }
        }
        Ok(())
    }
}

fn stage_file(target: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let temporary = target.with_file_name(format!("{name}.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    result.map_err(|error: std::io::Error| {
        let _ = fs::remove_file(&temporary);
        format!("无法保存配置 {name}：{error}")
    })?;
    Ok(temporary)
}

fn cleanup(paths: &[PathBuf]) {
    for path in paths {
        let _ = fs::remove_file(path);
    }
}

fn openssh_fingerprints(data: &str, profile: &Value) -> Vec<String> {
    let host = profile["host"].as_str().unwrap_or("");
    let target = if profile["port"] == 22 {
        host.to_owned()
    } else {
        format!("[{host}]:{}", profile["port"])
    };
    let mut found = Vec::new();
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(['#', '@']) {
            continue;
        }
        let columns = line.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 3 {
            continue;
        }
        let matches = columns[0].split(',').any(|host| {
            if host.starts_with("|1|") {
                let parts = host.split('|').collect::<Vec<_>>();
                if parts.len() != 4 {
                    return false;
                }
                let (Ok(salt), Ok(expected)) =
                    (STANDARD.decode(parts[2]), STANDARD.decode(parts[3]))
                else {
                    return false;
                };
                let Ok(mut hash) = Hmac::<Sha1>::new_from_slice(&salt) else {
                    return false;
                };
                hash.update(target.as_bytes());
                hash.verify_slice(&expected).is_ok()
            } else {
                host.eq_ignore_ascii_case(&target)
            }
        });
        if matches {
            let Ok(key) = STANDARD.decode(columns[2]) else {
                continue;
            };
            found.push(format!(
                "SHA256:{}",
                STANDARD_NO_PAD.encode(Sha256::digest(key))
            ));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let dir = std::env::current_dir()
                .unwrap()
                .join("test-output")
                .join(format!("lion-rust-store-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn open(&self) -> Store {
            Store::open(self.0.clone()).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn profile() -> Value {
        json!({"id":"saved", "name":"Fixture", "host":"192.0.2.10", "username":"user", "authType":"password", "tunnels":[]})
    }

    #[test]
    fn new_store_initializes_same_four_files_and_stays_empty() {
        let fixture = Fixture::new();
        assert!(fixture.open().public_profiles().is_empty());
        for name in FILES {
            assert!(fixture.0.join(name).is_file());
        }
        assert_eq!(fixture.open().settings, json!({"theme":"system"}));
    }

    #[test]
    fn existing_files_are_not_rewritten_and_unknown_fields_survive_updates() {
        let fixture = Fixture::new();
        let mut saved = profile();
        saved["extension"] = json!({"future":true});
        saved["gpu"] = json!({"newThreshold":12});
        saved["tunnels"] = json!([{"id":"tunnel", "direction":"remote", "future": "retained"}]);
        let bytes = json!([saved]).to_string();
        fs::write(fixture.0.join(FILES[0]), &bytes).unwrap();
        fs::write(
            fixture.0.join(FILES[1]),
            r#"{"saved":"opaque-legacy-value"}"#,
        )
        .unwrap();
        fs::write(
            fixture.0.join(FILES[3]),
            r#"{"theme":"dark","future":"retained"}"#,
        )
        .unwrap();
        let mut store = fixture.open();
        assert_eq!(fs::read_to_string(fixture.0.join(FILES[0])).unwrap(), bytes);
        assert_eq!(store.public_profiles()[0]["hasSavedSecret"], true);
        assert!(store.public_profiles()[0].get("extension").is_none());
        store.set_theme("light").unwrap();
        let saved: Value =
            serde_json::from_slice(&fs::read(fixture.0.join(FILES[0])).unwrap()).unwrap();
        assert_eq!(saved[0]["extension"], json!({"future":true}));
        assert_eq!(saved[0]["gpu"]["newThreshold"], 12);
        assert_eq!(saved[0]["tunnels"][0]["future"], "retained");
        assert_eq!(fixture.open().settings["future"], "retained");
        assert_eq!(fixture.open().secrets["saved"], "opaque-legacy-value");
    }

    #[test]
    fn corrupted_config_fails_without_replacing_it() {
        let fixture = Fixture::new();
        fs::write(fixture.0.join(FILES[0]), "{broken").unwrap();
        assert!(Store::open(fixture.0.clone())
            .err()
            .unwrap()
            .contains("无法读取配置 profiles.json"));
        assert_eq!(
            fs::read_to_string(fixture.0.join(FILES[0])).unwrap(),
            "{broken"
        );
        assert!(!fixture.0.join(FILES[1]).exists());
    }

    #[test]
    fn write_failure_rolls_back_memory_and_does_not_truncate_other_files() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        store.save(&profile(), None).unwrap();
        let previous = fs::read(fixture.0.join(FILES[0])).unwrap();
        fs::remove_file(fixture.0.join(FILES[3])).unwrap();
        fs::create_dir(fixture.0.join(FILES[3])).unwrap();
        assert!(store.remove("saved").is_err());
        assert_eq!(store.get("saved").unwrap()["host"], "192.0.2.10");
        assert!(store.set_theme("light").is_err());
        assert_eq!(store.settings["theme"], "system");
        let mut updated = profile();
        updated["host"] = json!("192.0.2.11");
        assert!(store.save(&updated, None).is_err());
        assert_eq!(store.get("saved").unwrap()["host"], "192.0.2.10");
        assert_eq!(fs::read(fixture.0.join(FILES[0])).unwrap(), previous);
        assert!(fs::read_dir(&fixture.0).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
    }

    #[test]
    fn legacy_or_invalid_secrets_are_never_treated_as_plaintext() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        store
            .secrets
            .insert("saved".into(), json!("legacy-encrypted-value"));
        assert!(store
            .read_secret("saved")
            .unwrap_err()
            .contains("重新输入密码或密钥口令"));
        assert_eq!(store.secrets["saved"], "legacy-encrypted-value");
        store
            .secrets
            .insert("saved".into(), json!("native:v1:not-base64!"));
        assert!(store.read_secret("saved").is_err());
        assert_eq!(store.read_secret("missing").unwrap(), json!({}));
    }

    #[cfg(windows)]
    #[test]
    fn existing_dpapi_format_round_trips_without_plaintext_on_disk() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let value = "fixture-password-秘密";
        store
            .save(
                &profile(),
                Some(&json!({"password":value, "remember":true})),
            )
            .unwrap();
        assert!(store.secrets["saved"]
            .as_str()
            .unwrap()
            .starts_with(SECRET_PREFIX));
        assert!(!fs::read_to_string(fixture.0.join(FILES[1]))
            .unwrap()
            .contains(value));
        assert_eq!(
            fixture.open().read_secret("saved").unwrap(),
            json!({"password":value})
        );
        store
            .save(&profile(), Some(&json!({"password":"", "remember":true})))
            .unwrap();
        assert_eq!(
            store.read_secret("saved").unwrap(),
            json!({"password":value})
        );
        store
            .save(&profile(), Some(&json!({"remember":false})))
            .unwrap();
        assert_eq!(fixture.open().read_secret("saved").unwrap(), json!({}));
    }

    #[test]
    fn openssh_plain_and_hashed_hosts_use_same_sha256_fingerprint() {
        let key = STANDARD.encode(b"fixture-public-key");
        let expected = format!(
            "SHA256:{}",
            STANDARD_NO_PAD.encode(Sha256::digest(b"fixture-public-key"))
        );
        let mut host = model::normalize_profile(&profile()).unwrap();
        let mut mac = Hmac::<Sha1>::new_from_slice(b"fixture-salt").unwrap();
        mac.update(b"[192.0.2.10]:2222");
        // OpenSSH hashed hostname format has no trailing delimiter.
        let hashed = format!(
            "|1|{}|{}",
            STANDARD.encode(b"fixture-salt"),
            STANDARD.encode(mac.finalize().into_bytes())
        );
        let data = format!("# comment\n192.0.2.10,example.test ssh-ed25519 {key}\n{hashed} ssh-rsa {key}\n@revoked 192.0.2.10 ssh-rsa {key}\n");
        assert_eq!(openssh_fingerprints(&data, &host), vec![expected.clone()]);
        host["port"] = json!(2222);
        assert_eq!(openssh_fingerprints(&data, &host), vec![expected]);
    }

    #[test]
    fn trusted_endpoint_array_is_persisted_in_existing_format() {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let profile = model::normalize_profile(&profile()).unwrap();
        store.trust(&profile, "SHA256:fixture").unwrap();
        assert_eq!(
            fixture.open().fingerprints(&profile).unwrap(),
            vec!["SHA256:fixture"]
        );
        let data: Value =
            serde_json::from_slice(&fs::read(fixture.0.join(FILES[2])).unwrap()).unwrap();
        assert_eq!(data, json!({"192.0.2.10:22":["SHA256:fixture"]}));
    }
}
