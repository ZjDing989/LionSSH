//! Download verified, complete official Codex packages before sending them over
//! SSH. Release metadata and all asset URLs are fixed to OpenAI's GitHub repo;
//! package bytes stream to private temporary files instead of a large Vec.
use reqwest::{Client, Url};
use semver::Version;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::File,
    path::{Path, PathBuf},
    time::Duration,
};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;

const METADATA_LIMIT: u64 = 8 * 1024 * 1024;
const SMALL_ASSET_LIMIT: u64 = 2 * 1024 * 1024;
const PACKAGE_LIMIT: u64 = 768 * 1024 * 1024;
const BUNDLE_LIMIT: u64 = 1024 * 1024 * 1024;
const INSTALLER_ASSET: &str = "install.sh";
const CHECKSUMS_ASSET: &str = "codex-package_SHA256SUMS";

pub struct NativePayload {
    pub archive_path: PathBuf,
    pub archive_size: u64,
    // Keep every source file and the resulting tar alive through SSH upload.
    _directory: TempDir,
}

#[derive(Debug)]
struct Asset {
    name: String,
    url: String,
    size: u64,
    digest: [u8; 32],
}

#[derive(Debug)]
struct Plan {
    installer: Asset,
    checksums: Asset,
    package: Asset,
}

fn package_name(target: &str) -> Result<String, String> {
    if ![
        "x86_64-unknown-linux-musl",
        "aarch64-unknown-linux-musl",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
    ]
    .contains(&target)
    {
        return Err("不支持该服务器的 Codex 原生安装平台".into());
    }
    Ok(format!("codex-package-{target}.tar.gz"))
}

fn tag_name(version: &Version) -> Result<String, String> {
    if !version.pre.is_empty() || !version.build.is_empty() {
        return Err("Codex 原生更新仅支持正式发布版本".into());
    }
    Ok(format!("rust-v{version}"))
}

fn digest(value: &str) -> Result<[u8; 32], String> {
    let hex = value.strip_prefix("sha256:").unwrap_or("");
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("官方发布包缺少有效的 SHA256 校验信息".into());
    }
    let mut result = [0; 32];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
            .map_err(|_| "官方发布包的 SHA256 校验信息无效")?;
    }
    Ok(result)
}

fn trusted_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && url.fragment().is_none()
        && matches!(
            url.host_str(),
            Some(
                "api.github.com"
                    | "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
}

fn plan(metadata: &[u8], version: &Version, target: &str) -> Result<Plan, String> {
    if metadata.len() as u64 > METADATA_LIMIT {
        return Err("Codex 官方发布信息过大".into());
    }
    let tag = tag_name(version)?;
    let package = package_name(target)?;
    let release: Value =
        serde_json::from_slice(metadata).map_err(|_| "无法解析 Codex 官方发布信息")?;
    if release["tag_name"].as_str() != Some(&tag)
        || release["draft"] != false
        || release["prerelease"] != false
    {
        return Err("Codex 官方发布版本与请求版本不一致".into());
    }
    let assets = release["assets"]
        .as_array()
        .ok_or("官方发布信息缺少文件列表")?;
    let mut names = HashSet::new();
    for asset in assets {
        let name = asset["name"].as_str().ok_or("官方发布文件名无效")?;
        if !names.insert(name) {
            return Err("官方发布信息包含重复文件".into());
        }
    }
    let select = |name: &str, limit: u64| -> Result<Asset, String> {
        let asset = assets
            .iter()
            .find(|asset| asset["name"] == name)
            .ok_or_else(|| format!("官方发布缺少 {name}"))?;
        let expected_url =
            format!("https://github.com/openai/codex/releases/download/{tag}/{name}");
        if asset["browser_download_url"].as_str() != Some(&expected_url) {
            return Err("Codex 发布包下载地址不属于所选官方版本".into());
        }
        let size = asset["size"]
            .as_u64()
            .filter(|size| *size > 0 && *size <= limit)
            .ok_or("Codex 官方发布包大小无效或超过限制")?;
        let digest = digest(asset["digest"].as_str().unwrap_or(""))?;
        Ok(Asset {
            name: name.into(),
            url: expected_url,
            size,
            digest,
        })
    };
    Ok(Plan {
        installer: select(INSTALLER_ASSET, SMALL_ASSET_LIMIT)?,
        checksums: select(CHECKSUMS_ASSET, SMALL_ASSET_LIMIT)?,
        package: select(&package, PACKAGE_LIMIT)?,
    })
}

fn client() -> Result<Client, String> {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(45))
        .timeout(Duration::from_secs(600))
        .user_agent(concat!("LionSSH/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 8 {
                attempt.error("官方发布包重定向次数过多")
            } else if trusted_url(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("拒绝非官方 HTTPS 下载地址")
            }
        }))
        .build()
        .map_err(|_| "无法初始化 Codex 更新包下载".into())
}

fn network_error(error: reqwest::Error) -> String {
    if error.is_timeout() {
        "下载 Codex 官方更新包超时，请检查本机网络后重试".into()
    } else {
        format!("下载 Codex 官方更新包失败：{}", error.without_url())
    }
}

struct Verifier {
    hash: Sha256,
    size: u64,
    expected_size: u64,
    expected_digest: [u8; 32],
}
impl Verifier {
    fn new(asset: &Asset) -> Self {
        Self {
            hash: Sha256::new(),
            size: 0,
            expected_size: asset.size,
            expected_digest: asset.digest,
        }
    }
    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.size = self
            .size
            .checked_add(bytes.len() as u64)
            .filter(|size| *size <= self.expected_size)
            .ok_or("Codex 更新包下载数据超过官方声明大小")?;
        self.hash.update(bytes);
        Ok(())
    }
    fn finish(self) -> Result<(), String> {
        if self.size != self.expected_size {
            return Err("Codex 更新包下载不完整，请重试".into());
        }
        let actual: [u8; 32] = self.hash.finalize().into();
        if actual != self.expected_digest {
            return Err("Codex 更新包 SHA256 校验失败，已停止安装".into());
        }
        Ok(())
    }
}

async fn download(client: &Client, asset: &Asset, destination: &Path) -> Result<(), String> {
    let mut response = client
        .get(&asset.url)
        .send()
        .await
        .map_err(network_error)?
        .error_for_status()
        .map_err(network_error)?;
    if response
        .content_length()
        .is_some_and(|size| size != asset.size)
    {
        return Err("Codex 更新包大小与官方发布信息不一致".into());
    }
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .await
        .map_err(|_| "无法创建 Codex 更新临时文件，请检查本机临时目录空间和权限")?;
    let mut verifier = Verifier::new(asset);
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        verifier.write(&chunk)?;
        file.write_all(&chunk)
            .await
            .map_err(|_| "无法写入 Codex 更新包，请检查本机磁盘空间")?;
    }
    file.flush().await.map_err(|_| "保存 Codex 更新包失败")?;
    verifier.finish()
}

fn verify_manifest(bytes: &[u8], package: &Asset) -> Result<(), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "Codex 官方校验文件格式无效")?;
    let matches: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            (fields.len() == 2 && fields[1] == package.name).then(|| fields[0])
        })
        .collect();
    if matches.len() != 1
        || digest(&format!("sha256:{}", matches.first().unwrap_or(&"")))? != package.digest
    {
        return Err("Codex 更新包校验清单与官方 SHA256 不一致".into());
    }
    Ok(())
}

fn bundle(directory: TempDir, plan: Plan) -> Result<NativePayload, String> {
    let archive_path = directory.path().join("codex-native-payload.tar");
    let file = File::create(&archive_path).map_err(|_| "无法创建 Codex 上传文件")?;
    let mut archive = tar::Builder::new(file);
    // Source contents never determine tar entry names or permissions. Never
    // recurse into a directory or include local credentials/user files.
    for (entry, source) in [
        ("installer.sh", INSTALLER_ASSET),
        ("release.json", "release.json"),
        (CHECKSUMS_ASSET, CHECKSUMS_ASSET),
        (plan.package.name.as_str(), plan.package.name.as_str()),
    ] {
        let mut file =
            File::open(directory.path().join(source)).map_err(|_| "读取 Codex 更新文件失败")?;
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o600);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_size(
            file.metadata()
                .map_err(|_| "读取 Codex 更新文件大小失败")?
                .len(),
        );
        archive
            .append_data(&mut header, entry, &mut file)
            .map_err(|_| "打包 Codex 更新文件失败")?;
    }
    let file = archive
        .into_inner()
        .map_err(|_| "完成 Codex 更新文件打包失败")?;
    let archive_size = file
        .metadata()
        .map_err(|_| "读取 Codex 上传文件大小失败")?
        .len();
    if archive_size == 0 || archive_size > BUNDLE_LIMIT {
        return Err("Codex 上传文件大小超过限制".into());
    }
    drop(file);
    Ok(NativePayload {
        archive_path,
        archive_size,
        _directory: directory,
    })
}

impl NativePayload {
    pub async fn fetch(version: &Version, target: &str) -> Result<Self, String> {
        // Validate caller data before any network access or temporary writes.
        package_name(target)?;
        let tag = tag_name(version)?;
        let client = client()?;
        let work = async {
            let mut response = client
                .get(format!(
                    "https://api.github.com/repos/openai/codex/releases/tags/{tag}"
                ))
                .header(reqwest::header::ACCEPT, "application/vnd.github+json")
                .timeout(Duration::from_secs(30))
                .send()
                .await
                .map_err(network_error)?
                .error_for_status()
                .map_err(network_error)?;
            if response
                .content_length()
                .is_some_and(|size| size > METADATA_LIMIT)
            {
                return Err("Codex 官方发布信息过大".into());
            }
            let mut metadata = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(network_error)? {
                if metadata.len() as u64 + chunk.len() as u64 > METADATA_LIMIT {
                    return Err("Codex 官方发布信息过大".into());
                }
                metadata.extend_from_slice(&chunk);
            }
            let plan = plan(&metadata, version, target)?;
            let directory = tempfile::Builder::new()
                .prefix("lion-codex-payload-")
                .tempdir()
                .map_err(|_| "无法创建 Codex 更新临时目录")?;
            tokio::fs::write(directory.path().join("release.json"), metadata)
                .await
                .map_err(|_| "保存 Codex 官方发布信息失败")?;
            for asset in [&plan.installer, &plan.checksums, &plan.package] {
                download(&client, asset, &directory.path().join(&asset.name)).await?;
                if asset.name == CHECKSUMS_ASSET {
                    let bytes = tokio::fs::read(directory.path().join(CHECKSUMS_ASSET))
                        .await
                        .map_err(|_| "读取 Codex 官方校验文件失败")?;
                    verify_manifest(&bytes, &plan.package)?;
                }
            }
            tokio::task::spawn_blocking(move || bundle(directory, plan))
                .await
                .map_err(|_| "Codex 更新包准备任务失败")?
        };
        tokio::time::timeout(Duration::from_secs(900), work)
            .await
            .map_err(|_| "准备 Codex 更新包超时，请检查本机网络后重试".to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Read;

    const TARGET: &str = "x86_64-unknown-linux-musl";
    const CONTENT: &[u8] = b"verified package bytes";
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
    fn metadata() -> Value {
        let assets = [INSTALLER_ASSET.to_string(),CHECKSUMS_ASSET.to_string(),package_name(TARGET).unwrap()].into_iter().map(|name| json!({
                "browser_download_url":format!("https://github.com/openai/codex/releases/download/rust-v0.162.1/{name}"),
                "name":name,"size":CONTENT.len(),"digest":format!("sha256:{}",hex(&Sha256::digest(CONTENT)))
            })).collect::<Vec<_>>();
        json!({"tag_name":"rust-v0.162.1","draft":false,"prerelease":false,"assets":assets})
    }
    fn fixture_plan() -> Plan {
        plan(
            &serde_json::to_vec(&metadata()).unwrap(),
            &Version::new(0, 162, 1),
            TARGET,
        )
        .unwrap()
    }

    #[test]
    fn metadata_requires_correct_tag_unique_fixed_assets_digest_and_size() {
        let version = Version::new(0, 162, 1);
        assert_eq!(
            fixture_plan().package.name,
            "codex-package-x86_64-unknown-linux-musl.tar.gz"
        );
        for mutation in 0..8 {
            let mut input = metadata();
            match mutation {
                0 => input["tag_name"] = json!("rust-v0.161.0"),
                1 => input["draft"] = json!(true),
                2 => input["assets"][2]["digest"] = Value::Null,
                3 => input["assets"][2]["digest"] = json!("sha256:bad"),
                4 => input["assets"][2]["size"] = json!(PACKAGE_LIMIT + 1),
                5 => {
                    input["assets"][2]["browser_download_url"] =
                        json!("https://github.com/other/repo/package.tar.gz")
                }
                6 => {
                    let duplicate = input["assets"][2].clone();
                    input["assets"].as_array_mut().unwrap().push(duplicate);
                }
                _ => {
                    input["assets"].as_array_mut().unwrap().remove(1);
                }
            }
            assert!(
                plan(&serde_json::to_vec(&input).unwrap(), &version, TARGET).is_err(),
                "mutation {mutation}"
            );
        }
        assert!(plan(b"not JSON", &version, TARGET).is_err());
        assert!(package_name("../../data").is_err());
        assert!(package_name("x86_64-unknown-linux-musl/../../bad").is_err());
        assert!(tag_name(&Version::parse("0.162.1+other").unwrap()).is_err());
    }

    #[test]
    fn official_https_redirects_exclude_credentials_other_hosts_ports_and_downgrades() {
        for url in [
            "https://github.com/openai/codex",
            "https://release-assets.githubusercontent.com/file?sig=fixture",
            "https://objects.githubusercontent.com/file",
        ] {
            assert!(trusted_url(&Url::parse(url).unwrap()));
        }
        for url in [
            "http://github.com/file",
            "https://github.com.evil.test/file",
            "https://evil.test/file",
            "https://user:pass@github.com/file",
            "https://github.com:444/file",
            "https://github.com/file#fragment",
        ] {
            assert!(!trusted_url(&Url::parse(url).unwrap()), "{url}");
        }
    }

    #[test]
    fn streamed_verification_rejects_tampering_truncation_and_extra_bytes() {
        let plan = fixture_plan();
        let mut verifier = Verifier::new(&plan.package);
        verifier.write(&CONTENT[..4]).unwrap();
        verifier.write(&CONTENT[4..]).unwrap();
        verifier.finish().unwrap();
        let mut verifier = Verifier::new(&plan.package);
        verifier.write(&vec![b'x'; CONTENT.len()]).unwrap();
        assert!(verifier.finish().unwrap_err().contains("SHA256"));
        let mut verifier = Verifier::new(&plan.package);
        verifier.write(&CONTENT[..4]).unwrap();
        assert!(verifier.finish().is_err());
        let mut verifier = Verifier::new(&plan.package);
        verifier.write(CONTENT).unwrap();
        assert!(verifier.write(b"extra").is_err());
    }

    #[test]
    fn checksum_manifest_must_match_the_selected_package_api_digest() {
        let plan = fixture_plan();
        let line = format!("{}  {}\n", hex(&plan.package.digest), plan.package.name);
        verify_manifest(line.as_bytes(), &plan.package).unwrap();
        assert!(verify_manifest(format!("{line}{line}").as_bytes(), &plan.package).is_err());
        assert!(verify_manifest(b"bad  other-package.tar.gz\n", &plan.package).is_err());
        assert!(verify_manifest(
            format!("{}  {}\n", "0".repeat(64), plan.package.name).as_bytes(),
            &plan.package
        )
        .is_err());
    }

    #[test]
    fn bundle_has_exactly_four_regular_safe_entries_and_drop_removes_everything() {
        let plan = fixture_plan();
        let directory = tempfile::tempdir().unwrap();
        let directory_path = directory.path().to_path_buf();
        for name in [
            INSTALLER_ASSET,
            "release.json",
            CHECKSUMS_ASSET,
            &plan.package.name,
            "credentials.json",
        ] {
            std::fs::write(directory.path().join(name), CONTENT).unwrap();
        }
        let payload = bundle(directory, plan).unwrap();
        assert!(payload.archive_size > 0 && payload.archive_path.is_file());
        let mut archive = tar::Archive::new(File::open(&payload.archive_path).unwrap());
        let mut names = Vec::new();
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            assert!(entry.header().entry_type().is_file());
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, CONTENT);
            names.push(entry.path().unwrap().to_string_lossy().into_owned());
        }
        assert_eq!(
            names,
            [
                "installer.sh",
                "release.json",
                CHECKSUMS_ASSET,
                "codex-package-x86_64-unknown-linux-musl.tar.gz"
            ]
        );
        drop(archive);
        drop(payload);
        assert!(!directory_path.exists());
    }
}
