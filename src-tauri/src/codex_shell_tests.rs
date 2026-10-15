//! Execute the generated POSIX scripts against an isolated fake server.  The
//! fixture PATH exposes only named utilities and fake Codex/package managers;
//! neither a developer's Codex installation nor a network installer can run.
use super::*;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

fn shell_path(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    {
        // std::fs::canonicalize expands 8.3 aliases and returns verbatim Win32
        // paths. Git's POSIX shell needs their ordinary drive/UNC spelling.
        if let Some(unc) = value.strip_prefix("//?/UNC/") {
            return format!("//{unc}");
        }
        let value = value.strip_prefix("//?/").unwrap_or(&value);
        if value.as_bytes().get(1) == Some(&b':') {
            return format!(
                "/{}/{}",
                value[..1].to_ascii_lowercase(),
                value[3..].trim_start_matches('/')
            );
        }
        return value.to_owned();
    }
    #[cfg(not(windows))]
    value
}

fn shell() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let mut candidates = vec![PathBuf::from(r"C:\Program Files\Git\usr\bin\sh.exe")];
        if let Some(programs) = std::env::var_os("ProgramFiles") {
            candidates.push(PathBuf::from(programs).join("Git/usr/bin/sh.exe"));
        }
        candidates.into_iter().find(|path| path.is_file())
    }
    #[cfg(not(windows))]
    {
        Some(PathBuf::from("/bin/sh"))
    }
}

struct Server {
    _root: tempfile::TempDir,
    home: PathBuf,
    bin: PathBuf,
    tools: PathBuf,
    shell: PathBuf,
}

impl Server {
    fn new() -> Option<Self> {
        let Some(shell) = shell() else {
            eprintln!("Codex shell fixtures require Git's POSIX sh on Windows");
            return None;
        };
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("server home's files");
        fs::create_dir_all(&home).unwrap();
        // Match the physical paths returned by the probe's `cd -P`: macOS
        // expands /var -> /private/var; Windows expands TEMP's RUNNER~1 alias.
        // Canonicalize the existing root once, before joining future paths.
        let home = fs::canonicalize(home).unwrap();
        let bin = home.join("fixture-bin");
        let tools = home.join("fixture-tools");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&tools).unwrap();
        fs::create_dir_all(home.join("private-transfer-temp")).unwrap();
        let server = Self {
            _root: root,
            home,
            bin,
            tools,
            shell,
        };
        for name in [
            "od", "tr", "dirname", "basename", "readlink", "sed", "awk", "id", "mktemp", "tar",
            "env", "sh", "rm", "cp", "grep", "cat", "mkdir", "chmod",
        ] {
            #[cfg(windows)]
            let target = format!("/usr/bin/{name}");
            #[cfg(not(windows))]
            let target = if Path::new(&format!("/usr/bin/{name}")).exists() {
                format!("/usr/bin/{name}")
            } else {
                format!("/bin/{name}")
            };
            server.write(
                &server.tools.join(name),
                &format!("#!/bin/sh\nexec {} \"$@\"\n", quote(&target)),
            );
        }
        // These fake commands make probes deterministic and block all network
        // operations even if a generated script accidentally invokes a fetch.
        server.write(
            &server.tools.join("uname"),
            "#!/bin/sh\ncase \"${1-}\" in -s) echo Linux;; -m) echo x86_64;; *) exit 1;; esac\n",
        );
        server.write(&server.tools.join("pgrep"), "#!/bin/sh\nexit 1\n");
        server.write(&server.tools.join("systemctl"), "#!/bin/sh\nexit 0\n");
        server.write(&server.tools.join("sleep"), "#!/bin/sh\nexit 0\n");
        server.write(
            &server.tools.join("curl"),
            "#!/bin/sh\necho 'fixture refuses real downloads' >&2\nexit 91\n",
        );
        server.write(
            &server.tools.join("wget"),
            "#!/bin/sh\necho 'fixture refuses real downloads' >&2\nexit 91\n",
        );
        Some(server)
    }

    fn write(&self, path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    fn process(&self) -> Command {
        let mut command = Command::new(&self.shell);
        command.env_clear();
        for name in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .env("HOME", shell_path(&self.home))
            .env(
                "PATH",
                format!("{}:{}", shell_path(&self.bin), shell_path(&self.tools)),
            )
            .env(
                "TMPDIR",
                shell_path(&self.home.join("private-transfer-temp")),
            )
            .env("SHELL", "/bin/sh")
            .current_dir(&self.home);
        command
    }

    fn run(&self, script: &str) -> Output {
        let mut command = self.process();
        command
            .arg("-s")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn probe(&self) -> Probe {
        let output = self.run(PROBE);
        assert!(
            output.status.success(),
            "probe: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Probe::parse(&String::from_utf8(output.stdout).unwrap()).unwrap()
    }

    fn fixture_probe(&self, method: &str) -> Probe {
        Probe {
            installed: true,
            current: Some(Version::new(0, 161, 0)),
            path: shell_path(&self.bin.join("codex")),
            real: shell_path(&self.bin.join("codex-native")),
            method: method.into(),
            native_home: shell_path(&self.home.join(".codex")),
            bin_dir: shell_path(&self.bin),
            npm: shell_path(&self.bin.join("npm")),
            npm_root: shell_path(&self.home.join("node/lib/node_modules")),
            npm_prefix: shell_path(&self.home.join("node")),
            allowed: true,
            reason: None,
            os: "Linux".into(),
            arch: "x86_64".into(),
        }
    }

    fn run_input(&self, script: &str, input: &[u8]) -> Output {
        let mut command = self.process();
        command
            .arg("-c")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let _ = child.stdin.take().unwrap().write_all(input);
        child.wait_with_output().unwrap()
    }

    fn native_transfer_probe(&self) -> Probe {
        let mut probe = self.fixture_probe("native");
        probe.native_home = shell_path(&self.home.join("custom codex home's config"));
        probe.bin_dir = shell_path(&self.home.join("custom user's bin"));
        probe
    }

    fn assert_transfer_cleanup(&self) {
        let entries: Vec<_> = fs::read_dir(self.home.join("private-transfer-temp"))
            .unwrap()
            .collect();
        assert!(
            entries.is_empty(),
            "temporary upload directory was not removed"
        );
    }
}

#[cfg(windows)]
#[test]
fn windows_shell_paths_accept_canonical_verbatim_disks_and_unc_paths() {
    for input in [
        r"C:\Users\runneradmin\server home's files\node prefix",
        r"\\?\C:\Users\runneradmin\server home's files\node prefix",
    ] {
        assert_eq!(
            shell_path(Path::new(input)),
            "/c/Users/runneradmin/server home's files/node prefix"
        );
    }
    assert_eq!(
        shell_path(Path::new(r"\\?\UNC\server\share\server home's files")),
        "//server/share/server home's files"
    );
}

#[test]
fn all_generated_remote_scripts_are_valid_posix_shell() {
    let Some(server) = Server::new() else {
        return;
    };
    let native = server.fixture_probe("native");
    let npm = server.fixture_probe("npm");
    let plan = RestartPlan {
        unit: "codex-test.service".into(),
        pid: 1234,
        started: 98765,
    };
    for script in [
        PROBE.to_string(),
        runtime_script(&native),
        install_script(&native, &Version::new(0, 162, 1)).unwrap(),
        install_script(&npm, &Version::new(0, 162, 1)).unwrap(),
        restart_script(&plan, &native, &native),
        daemon_script(&native, "version"),
        daemon_script(&native, "update"),
    ] {
        let mut command = server.process();
        command
            .arg("-n")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(
            result.status.success(),
            "invalid script:\n{script}\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn actual_probe_distinguishes_absent_and_unrecognized_installations() {
    let Some(server) = Server::new() else {
        return;
    };
    let absent = server.probe();
    assert!(!absent.installed);
    assert_eq!(absent.method, "native");
    assert!(absent.allowed);
    assert_eq!(absent.native_home, shell_path(&server.home.join(".codex")));

    server.write(
        &server.bin.join("codex"),
        "#!/bin/sh\nprintf 'codex-cli 0.161.0\\n'\n",
    );
    let unknown = server.probe();
    assert!(unknown.installed);
    assert_eq!(unknown.current, Some(Version::new(0, 161, 0)));
    assert_eq!(unknown.method, "unknown");
    assert!(!unknown.allowed);
}

#[test]
fn actual_native_transfer_preserves_custom_paths_and_serves_only_local_package_files() {
    let Some(server) = Server::new() else {
        return;
    };
    let probe = server.native_transfer_probe();
    let marker = server.home.join("installer-environment");
    let script = format!(
        r#"#!/bin/sh
set -eu
stage=$(dirname "$0")
metadata=$(curl -fsSL https://api.github.com/repos/openai/codex/releases/latest)
[ "$metadata" = '{{"tag_name":"rust-v0.162.1","assets":[]}}' ] || exit 81
[ "$(curl -fsSL https://api.github.com/repos/openai/codex/releases/tags/rust-v0.162.1)" = "$metadata" ] || exit 82
curl -fsSL --connect-timeout 10 --max-time 30 https://github.com/openai/codex/releases/download/rust-v0.162.1/codex-package_SHA256SUMS -o "$stage/copied-checksums"
[ "$(cat "$stage/copied-checksums")" = 'fixture-checksums' ] || exit 83
curl -fsSL --proto '=https' --proto-redir '=https' -o "$stage/copied-package" https://github.com/openai/codex/releases/download/rust-v0.162.1/codex-package-x86_64-unknown-linux-musl.tar.gz
[ "$(cat "$stage/copied-package")" = 'COMPLETE-FIXTURE-PACKAGE' ] || exit 84
printf '%s\n' "$CODEX_NON_INTERACTIVE" "$CODEX_RELEASE" "$CODEX_INSTALL_DIR" "$CODEX_HOME" "$CODEX_INSTALL_DAEMON_ONLY" "$CODEX_INSTALL_DEFER_SELECTION" "$CODEX_INSTALL_IF_LATEST" "$CODEX_INSTALL_IF_CURRENT" "$CODEX_INSTALLER_USE_RELEASES_OPENAI_COM" > {marker}
"#,
        marker = quote(&shell_path(&marker))
    );
    let output = server.run_input(
        &install_script(&probe, &Version::new(0, 162, 1)).unwrap(),
        &transfer_archive(&script, None),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(marker)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [
            "1",
            "latest",
            &probe.bin_dir,
            &probe.native_home,
            "0",
            "0",
            "0",
            "0",
            "0"
        ]
    );
    server.assert_transfer_cleanup();
}

fn transfer_archive(installer: &str, omitted: Option<&str>) -> Vec<u8> {
    let mut archive = tar::Builder::new(Vec::new());
    for (name, bytes) in [
        ("installer.sh", installer.as_bytes()),
        (
            "release.json",
            b"{\"tag_name\":\"rust-v0.162.1\",\"assets\":[]}".as_slice(),
        ),
        ("codex-package_SHA256SUMS", b"fixture-checksums".as_slice()),
        (
            "codex-package-x86_64-unknown-linux-musl.tar.gz",
            b"COMPLETE-FIXTURE-PACKAGE".as_slice(),
        ),
    ] {
        if omitted == Some(name) {
            continue;
        }
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o600);
        header.set_size(bytes.len() as u64);
        archive.append_data(&mut header, name, bytes).unwrap();
    }
    archive.into_inner().unwrap()
}

#[test]
fn actual_native_transfer_missing_or_truncated_payload_never_executes_installer() {
    for truncated in [false, true] {
        let Some(server) = Server::new() else {
            return;
        };
        let probe = server.native_transfer_probe();
        let marker = server.home.join("must-not-execute");
        let installer = format!("#!/bin/sh\nprintf ran > {}\n", quote(&shell_path(&marker)));
        let mut input = transfer_archive(
            &installer,
            if truncated {
                None
            } else {
                Some("codex-package_SHA256SUMS")
            },
        );
        if truncated {
            input.truncate(700);
        }
        let output = server.run_input(
            &install_script(&probe, &Version::new(0, 162, 1)).unwrap(),
            &input,
        );
        assert!(!output.status.success());
        assert!(!marker.exists(), "incomplete transfer executed installer");
        if !truncated {
            assert_eq!(output.status.code(), Some(65));
        }
        server.assert_transfer_cleanup();
    }
}

#[test]
fn actual_native_transfer_installer_failure_cleans_private_stage_and_preserves_existing_codex() {
    let Some(server) = Server::new() else {
        return;
    };
    let probe = server.native_transfer_probe();
    let existing = PathBuf::from(&probe.bin_dir).join("codex");
    // Build the native Windows path independently from the shell spelling.
    let existing = if cfg!(windows) {
        server.home.join("custom user's bin/codex")
    } else {
        existing
    };
    server.write(&existing, "EXISTING-CODEX");
    let marker = server.home.join("installer-entered");
    let installer = format!(
        "#!/bin/sh\nprintf ran > {}\necho 'fixture installer failed' >&2\nexit 42\n",
        quote(&shell_path(&marker))
    );
    let output = server.run_input(
        &install_script(&probe, &Version::new(0, 162, 1)).unwrap(),
        &transfer_archive(&installer, None),
    );
    assert_eq!(output.status.code(), Some(42));
    assert!(marker.exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("fixture installer failed"));
    assert_eq!(fs::read_to_string(existing).unwrap(), "EXISTING-CODEX");
    server.assert_transfer_cleanup();
}

#[test]
fn actual_native_transfer_rejects_unexpected_download_urls_without_any_network_fallback() {
    for url in [
        "https://example.invalid/installer.sh",
        "https://github.com/openai/codex/releases/download/rust-v0.161.0/codex-package_SHA256SUMS",
        "https://github.com/openai/codex/releases/download/rust-v0.162.1/codex-package-aarch64-unknown-linux-musl.tar.gz",
    ] {
        let Some(server) = Server::new() else { return; };
        let probe=server.native_transfer_probe();
        let marker=server.home.join("must-not-complete");
        let installer=format!("#!/bin/sh\nset -eu\ncurl -fsSL {}\nprintf ran > {}\n",quote(url),quote(&shell_path(&marker)));
        let output=server.run_input(&install_script(&probe,&Version::new(0,162,1)).unwrap(),&transfer_archive(&installer,None));
        assert_eq!(output.status.code(),Some(64));
        assert!(!marker.exists());
        assert!(String::from_utf8_lossy(&output.stderr).contains("拒绝访问完整安装包以外的下载地址"));
        server.assert_transfer_cleanup();
    }
}

#[test]
fn actual_runtime_and_daemon_scripts_use_only_fixture_commands() {
    let Some(server) = Server::new() else {
        return;
    };
    let probe = server.fixture_probe("native");
    let output = server.run(&runtime_script(&probe));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (plan, running) = runtime(&String::from_utf8(output.stdout).unwrap()).unwrap();
    assert!(plan.is_none());
    assert_eq!(running, "no");

    let marker = server.home.join("daemon-call");
    server.write(
        &server.bin.join("codex"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$CODEX_HOME\" \"$@\" > {}\n",
            quote(&shell_path(&marker))
        ),
    );
    let output = server.run(&daemon_script(&probe, "update"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(marker).unwrap(),
        format!("{}\napp-server\ndaemon\nupdate\n", probe.native_home)
    );
}

#[test]
fn actual_probe_and_install_preserve_the_identified_npm_global_prefix() {
    let Some(mut server) = Server::new() else {
        return;
    };
    let prefix = server.home.join("node prefix");
    let root = prefix.join("lib/node_modules");
    let package = root.join("@openai/codex");
    // A direct package entrypoint also tests detection without relying on
    // Windows symlink privileges. Unix global npm links resolve to this path.
    server.bin = package.join("bin");
    server.write(
        &server.bin.join("codex"),
        "#!/bin/sh\nprintf 'codex-cli 0.161.0\\n'\n",
    );
    server.write(
        &package.join("package.json"),
        "{\"name\":\"@openai/codex\",\"version\":\"0.161.0\"}\n",
    );
    let marker = server.home.join("npm-call");
    server.write(
        &server.bin.join("npm"),
        &format!(
            r#"#!/bin/sh
case "$1:${{2-}}" in
  root:-g) printf '%s\n' {root};;
  prefix:-g) printf '%s\n' {prefix};;
  install:--global) printf '%s\n' "$@" > {marker};;
  *) exit 9;;
esac
"#,
            root = quote(&shell_path(&root)),
            prefix = quote(&shell_path(&prefix)),
            marker = quote(&shell_path(&marker))
        ),
    );
    // The production node command only parses this metadata. A fixture node
    // checks the supplied metadata and records no code from the package.
    server.write(&server.bin.join("node"), "#!/bin/sh\nfor argument in \"$@\"; do metadata=$argument; done\ngrep -Eq '\"name\"[[:space:]]*:[[:space:]]*\"@openai/codex\"' \"$metadata\"\n");
    let probe = server.probe();
    assert!(probe.installed && probe.allowed);
    assert_eq!(probe.method, "npm");
    assert_eq!(probe.npm_root, shell_path(&root));
    assert_eq!(probe.npm_prefix, shell_path(&prefix));
    let output = server.run(&install_script(&probe, &Version::new(0, 162, 1)).unwrap());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let arguments = fs::read_to_string(marker).unwrap();
    assert_eq!(
        arguments.lines().collect::<Vec<_>>(),
        [
            "install",
            "--global",
            "--prefix",
            probe.npm_prefix.as_str(),
            "--no-audit",
            "--no-fund",
            "--",
            "@openai/codex@0.162.1"
        ]
    );

    server.write(
        &package.join("package.json"),
        "{\"name\":\"unrelated-tool\"}\n",
    );
    let unrelated = server.probe();
    assert_eq!(unrelated.method, "unknown");
    assert!(!unrelated.allowed);
}

#[cfg(unix)]
#[test]
fn actual_probe_follows_official_native_current_and_visible_symlinks() {
    use std::os::unix::fs::symlink;
    let Some(server) = Server::new() else {
        return;
    };
    let native = server.home.join(".codex/packages/standalone");
    let release = native.join("releases/0.161.0-x86_64-unknown-linux-musl");
    server.write(
        &release.join("bin/codex"),
        "#!/bin/sh\nprintf 'codex-cli 0.161.0\\n'\n",
    );
    symlink(&release, native.join("current")).unwrap();
    fs::create_dir_all(server.home.join(".local/bin")).unwrap();
    symlink(
        native.join("current/bin/codex"),
        server.home.join(".local/bin/codex"),
    )
    .unwrap();
    let probe = server.probe();
    assert!(probe.installed && probe.allowed);
    assert_eq!(probe.method, "native");
    assert_eq!(probe.real, shell_path(&release.join("bin/codex")));
    assert_eq!(probe.current, Some(Version::new(0, 161, 0)));
}

#[cfg(unix)]
#[test]
fn actual_probe_finds_one_nvm_install_and_rejects_ambiguous_versions() {
    use std::os::unix::fs::symlink;
    let Some(server) = Server::new() else {
        return;
    };
    let prefix = server.home.join(".nvm/versions/node/v22.1.0");
    let root = prefix.join("lib/node_modules");
    let package = root.join("@openai/codex");
    let bin = prefix.join("bin");
    server.write(
        &package.join("bin/codex.js"),
        "#!/bin/sh\nprintf 'codex-cli 0.161.0\\n'\n",
    );
    server.write(
        &package.join("package.json"),
        "{\"name\":\"@openai/codex\"}\n",
    );
    fs::create_dir_all(&bin).unwrap();
    symlink(package.join("bin/codex.js"), bin.join("codex")).unwrap();
    server.write(
        &bin.join("npm"),
        &format!(
            "#!/bin/sh\ncase \"$1:$2\" in root:-g) printf '%s\\n' {};; prefix:-g) printf '%s\\n' {};; *) exit 9;; esac\n",
            quote(&shell_path(&root)),
            quote(&shell_path(&prefix))
        ),
    );
    server.write(
        &bin.join("node"),
        "#!/bin/sh\nfor argument in \"$@\"; do metadata=$argument; done\ngrep -Eq '\"name\"[[:space:]]*:[[:space:]]*\"@openai/codex\"' \"$metadata\"\n",
    );
    let probe = server.probe();
    assert!(probe.installed && probe.allowed);
    assert_eq!(probe.method, "npm");
    assert_eq!(probe.path, shell_path(&bin.join("codex")));
    assert_eq!(probe.npm_prefix, shell_path(&prefix));

    server.write(
        &server.home.join(".nvm/versions/node/v24.1.0/bin/codex"),
        "#!/bin/sh\nprintf 'codex-cli 0.161.0\\n'\n",
    );
    let output = server.run(PROBE);
    assert!(output.status.success());
    let error = Probe::parse(&String::from_utf8(output.stdout).unwrap()).unwrap_err();
    assert!(error.contains("多个"), "{error}");
}
