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
    if value.as_bytes().get(1) == Some(&b':') {
        return format!(
            "/{}/{}",
            value[..1].to_ascii_lowercase(),
            value[3..].trim_start_matches('/')
        );
    }
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
        // macOS commonly exposes TMPDIR through /var -> /private/var. Match
        // the canonical paths returned by the remote probe's `cd -P`.
        #[cfg(unix)]
        let home = fs::canonicalize(home).unwrap();
        let bin = home.join("fixture-bin");
        let tools = home.join("fixture-tools");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&tools).unwrap();
        let server = Self {
            _root: root,
            home,
            bin,
            tools,
            shell,
        };
        for name in [
            "od", "tr", "dirname", "basename", "readlink", "sed", "awk", "id", "mktemp", "tar",
            "env", "sh", "rm", "cp", "grep",
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
        }
    }
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
fn actual_native_install_script_uses_only_fake_download_and_expected_environment() {
    let Some(server) = Server::new() else {
        return;
    };
    let probe = server.probe();
    let marker = server.home.join("installer-environment");
    let payload = server.home.join("fake-installer.sh");
    server.write(&payload, &format!("#!/bin/sh\nprintf '%s\\n' \"$CODEX_NON_INTERACTIVE\" \"$CODEX_RELEASE\" \"$CODEX_INSTALL_DIR\" \"$CODEX_HOME\" \"$CODEX_INSTALL_DAEMON_ONLY\" > {}\n", quote(&shell_path(&marker))));
    server.write(&server.tools.join("curl"), &format!("#!/bin/sh\noutput=''\nwhile [ \"$#\" -gt 0 ]; do if [ \"$1\" = -o ]; then shift; output=$1; fi; shift; done\n[ -n \"$output\" ] || exit 2\ncp {} \"$output\"\n", quote(&shell_path(&payload))));
    let output = server.run(&install_script(&probe, &Version::new(0, 162, 1)).unwrap());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let observed = fs::read_to_string(marker).unwrap();
    let values: Vec<_> = observed.lines().collect();
    assert_eq!(
        values,
        [
            "1",
            "latest",
            probe.bin_dir.as_str(),
            probe.native_home.as_str(),
            "0"
        ]
    );
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
