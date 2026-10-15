//! Native credential protection. Windows keeps its existing DPAPI wire format;
//! macOS/Linux use an authenticated envelope whose key lives in the OS keyring.
//! There is no plaintext or local-file master-key fallback.
use base64::{engine::general_purpose::STANDARD, Engine};

const KEYRING_MAGIC: &[u8] = b"LION-KC\x01";
const FOREIGN_CREDENTIAL: &str = "此凭据属于其他操作系统，请在当前系统重新输入并保存密码或密钥口令";

pub fn transform(method: &str, value: &str) -> Result<String, String> {
    match method {
        "encrypt" => protect(value.as_bytes()).map(|bytes| STANDARD.encode(bytes)),
        "decrypt" => {
            let encrypted = STANDARD
                .decode(value)
                .map_err(|_| "保存的凭据格式无效".to_string())?;
            let bytes = unprotect(&encrypted)?;
            String::from_utf8(bytes).map_err(|_| "解密后的凭据格式无效".to_string())
        }
        _ => Err("不支持的原生凭据操作".into()),
    }
}

#[cfg(windows)]
fn dpapi(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use std::{ptr, slice};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    };
    let length = u32::try_from(bytes.len()).map_err(|_| "凭据过长".to_string())?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: length,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    // DPAPI allocates output with LocalAlloc. Always copy and release that buffer.
    let ok = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 {
        return Err(if encrypt {
            "Windows 无法安全保存凭据"
        } else {
            "Windows 无法解密凭据，请使用保存时的 Windows 账户，或重新输入凭据"
        }
        .into());
    }
    let result = unsafe {
        let value = if output.cbData == 0 {
            Vec::new()
        } else {
            slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
        };
        LocalFree(output.pbData as _);
        value
    };
    Ok(result)
}

#[cfg(windows)]
fn protect(bytes: &[u8]) -> Result<Vec<u8>, String> {
    dpapi(bytes, true)
}
#[cfg(windows)]
fn unprotect(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.starts_with(KEYRING_MAGIC) {
        return Err(FOREIGN_CREDENTIAL.into());
    }
    dpapi(bytes, false)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn protect(bytes: &[u8]) -> Result<Vec<u8>, String> {
    envelope::seal(
        &system_keyring::master_key(true)?,
        bytes,
        envelope::platform(),
    )
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn unprotect(bytes: &[u8]) -> Result<Vec<u8>, String> {
    // Reject DPAPI or another platform before consulting the current keyring.
    envelope::validate(bytes, envelope::platform())?;
    envelope::open(
        &system_keyring::master_key(false)?,
        bytes,
        envelope::platform(),
    )
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn protect(_: &[u8]) -> Result<Vec<u8>, String> {
    Err("此平台不支持系统安全凭据存储；可以使用不记住密码的会话".into())
}
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn unprotect(_: &[u8]) -> Result<Vec<u8>, String> {
    Err("此平台不支持系统安全凭据存储，请重新输入密码或密钥口令".into())
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
mod envelope {
    use super::{FOREIGN_CREDENTIAL, KEYRING_MAGIC};
    use aes_gcm::{
        aead::{Aead, KeyInit, Payload},
        Aes256Gcm, Nonce,
    };
    use rand::TryRngCore;

    pub const MACOS: u8 = 1;
    pub const LINUX: u8 = 2;
    const NONCE_LENGTH: usize = 12;
    const TAG_LENGTH: usize = 16;
    const HEADER_LENGTH: usize = KEYRING_MAGIC.len() + 1;

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub fn platform() -> u8 {
        if cfg!(target_os = "macos") {
            MACOS
        } else {
            LINUX
        }
    }

    fn random<const N: usize>() -> Result<[u8; N], String> {
        let mut bytes = [0; N];
        rand::rngs::OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| "系统安全随机数不可用，无法安全保存凭据".to_string())?;
        Ok(bytes)
    }

    pub fn validate(bytes: &[u8], platform: u8) -> Result<(), String> {
        if !bytes.starts_with(KEYRING_MAGIC) || bytes.get(KEYRING_MAGIC.len()) != Some(&platform) {
            return Err(FOREIGN_CREDENTIAL.into());
        }
        if bytes.len() < HEADER_LENGTH + NONCE_LENGTH + TAG_LENGTH {
            return Err("保存的凭据格式无效".into());
        }
        Ok(())
    }

    pub fn seal(key: &[u8; 32], plaintext: &[u8], platform: u8) -> Result<Vec<u8>, String> {
        let cipher =
            Aes256Gcm::new_from_slice(key).map_err(|_| "系统凭据密钥格式无效".to_string())?;
        let mut output = KEYRING_MAGIC.to_vec();
        output.push(platform);
        let nonce = random::<NONCE_LENGTH>()?;
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &output,
                },
            )
            .map_err(|_| "系统无法安全保存凭据".to_string())?;
        output.extend_from_slice(&nonce);
        output.extend_from_slice(&ciphertext);
        Ok(output)
    }

    pub fn open(key: &[u8; 32], bytes: &[u8], platform: u8) -> Result<Vec<u8>, String> {
        validate(bytes, platform)?;
        let cipher =
            Aes256Gcm::new_from_slice(key).map_err(|_| "系统凭据密钥格式无效".to_string())?;
        cipher
            .decrypt(
                Nonce::from_slice(&bytes[HEADER_LENGTH..HEADER_LENGTH + NONCE_LENGTH]),
                Payload {
                    msg: &bytes[HEADER_LENGTH + NONCE_LENGTH..],
                    aad: &bytes[..HEADER_LENGTH],
                },
            )
            .map_err(|_| "系统无法解密凭据，请使用保存时的系统账户及密钥环，或重新输入凭据".into())
    }

    // Called under an interprocess file lock in production. Injected storage
    // keeps tests deterministic and avoids opening any real OS keychain.
    pub fn load_key(
        read: impl Fn() -> Result<Option<Vec<u8>>, String>,
        write: impl Fn(&[u8]) -> Result<(), String>,
        create: bool,
    ) -> Result<[u8; 32], String> {
        let bytes = match read()? {
            Some(bytes) => bytes,
            None if create => {
                let key = random::<32>()?;
                write(&key)?;
                read()?.ok_or_else(|| "系统密钥环未保存凭据密钥，请解锁密钥环后重试".to_string())?
            }
            None => {
                return Err(
                    "系统密钥环中未找到凭据密钥，请使用保存时的系统账户，或重新输入凭据".into(),
                )
            }
        };
        bytes
            .try_into()
            .map_err(|_| "系统密钥环中的凭据密钥格式无效，原密钥不会被自动覆盖".into())
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod system_keyring {
    use super::envelope;
    use std::fs::{DirBuilder, OpenOptions};
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    const SERVICE: &str = "tools.lion.ssh";
    const ACCOUNT: &str = "credential-encryption-key-v1";
    const UNAVAILABLE: &str = "系统密钥环不可用或已锁定，无法记住密码；请解锁 macOS 钥匙串或 Linux Secret Service，或使用不记住密码的会话";

    pub fn master_key(create: bool) -> Result<[u8; 32], String> {
        let directory = dirs::data_dir()
            .ok_or("无法确定系统凭据目录")?
            .join(SERVICE);
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)
            .map_err(|_| "无法创建系统凭据锁目录".to_string())?;
        // All data directories and verification instances of this OS account
        // share the same lock and key. A missing key is never created on read.
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(directory.join(".credential-key-v1.lock"))
            .map_err(|_| "无法打开系统凭据锁".to_string())?;
        lock.lock()
            .map_err(|_| "无法锁定系统凭据存储".to_string())?;
        let entry = keyring::Entry::new(SERVICE, ACCOUNT).map_err(|_| UNAVAILABLE.to_string())?;
        envelope::load_key(
            || match entry.get_secret() {
                Ok(bytes) => Ok(Some(bytes)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(_) => Err(UNAVAILABLE.into()),
            },
            |bytes| entry.set_secret(bytes).map_err(|_| UNAVAILABLE.to_string()),
            create,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_ciphertext() {
        assert!(transform("decrypt", "not-base64!").is_err());
    }
    #[test]
    fn rejects_unknown_crypto_operation() {
        assert!(transform("export", "secret").is_err());
    }
    #[cfg(windows)]
    #[test]
    fn protected_credentials_round_trip_and_reject_tampering() {
        let input = "密钥 passphrase 🔒";
        let ciphertext = transform("encrypt", input).unwrap();
        assert_ne!(ciphertext, STANDARD.encode(input));
        assert_eq!(transform("decrypt", &ciphertext).unwrap(), input);
        let mut bytes = STANDARD.decode(ciphertext).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x80;
        assert!(transform("decrypt", &STANDARD.encode(bytes)).is_err());
    }

    #[test]
    fn keyring_envelopes_are_randomized_authenticated_and_platform_bound() {
        let key = [0x42; 32];
        let plaintext = "密钥 passphrase 🔒".as_bytes();
        for platform in [envelope::MACOS, envelope::LINUX] {
            let bytes = envelope::seal(&key, plaintext, platform).unwrap();
            assert_ne!(bytes, plaintext);
            assert!(!bytes
                .windows(plaintext.len())
                .any(|value| value == plaintext));
            assert_ne!(envelope::seal(&key, plaintext, platform).unwrap(), bytes);
            assert_eq!(envelope::open(&key, &bytes, platform).unwrap(), plaintext);
            let mut tampered = bytes.clone();
            let last = tampered.len() - 1;
            tampered[last] ^= 0x80;
            assert!(envelope::open(&key, &tampered, platform).is_err());
            assert!(envelope::open(&[0x43; 32], &bytes, platform).is_err());
            let other = if platform == envelope::MACOS {
                envelope::LINUX
            } else {
                envelope::MACOS
            };
            assert_eq!(
                envelope::open(&key, &bytes, other).unwrap_err(),
                FOREIGN_CREDENTIAL
            );
        }
    }

    #[test]
    fn keyring_read_never_creates_missing_or_replaces_invalid_keys() {
        use std::cell::RefCell;
        let stored = RefCell::new(None::<Vec<u8>>);
        let reads = || Ok(stored.borrow().clone());
        let writes = |bytes: &[u8]| {
            *stored.borrow_mut() = Some(bytes.to_vec());
            Ok(())
        };
        assert!(envelope::load_key(reads, writes, false)
            .unwrap_err()
            .contains("未找到"));
        assert!(stored.borrow().is_none());
        let first = envelope::load_key(reads, writes, true).unwrap();
        assert_eq!(envelope::load_key(reads, writes, false).unwrap(), first);
        *stored.borrow_mut() = Some(vec![1, 2, 3]);
        assert!(envelope::load_key(reads, writes, true)
            .unwrap_err()
            .contains("不会被自动覆盖"));
        assert_eq!(*stored.borrow(), Some(vec![1, 2, 3]));
        assert_eq!(
            envelope::load_key(|| Err("locked keyring".into()), writes, true).unwrap_err(),
            "locked keyring"
        );
        assert_eq!(*stored.borrow(), Some(vec![1, 2, 3]));
    }

    #[test]
    fn keyring_envelopes_reject_dpapi_truncation_and_header_tampering() {
        let key = [0x42; 32];
        let mut bytes = envelope::seal(&key, b"secret", envelope::MACOS).unwrap();
        assert!(
            envelope::validate(&bytes[..KEYRING_MAGIC.len() + 1], envelope::MACOS)
                .unwrap_err()
                .contains("格式无效")
        );
        assert_eq!(
            envelope::validate(b"\x01\x00\x00\x00Windows-DPAPI", envelope::MACOS).unwrap_err(),
            FOREIGN_CREDENTIAL
        );
        bytes[KEYRING_MAGIC.len()] = envelope::LINUX;
        assert!(
            envelope::open(&key, &bytes, envelope::LINUX).is_err(),
            "authenticated platform header cannot be changed"
        );
        #[cfg(windows)]
        assert_eq!(unprotect(&bytes).unwrap_err(), FOREIGN_CREDENTIAL);
    }
}
