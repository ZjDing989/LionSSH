<p align="center"><a href="README.md">简体中文</a> · <strong>English</strong></p>

<h1 align="center">Lion SSH | <img src="assets/icon.png" alt="Lion SSH Logo" width="40" height="40" /></h1>

<p align="center">Manage multiple SSH connections, port tunnels, GPU monitoring, and Slurm jobs in one desktop workspace.</p>

Built with **Tauri 2 + Rust + Vue 3 + TypeScript**. SSH, tunnels, monitoring, configuration storage, and credential encryption run inside the Rust application. Release packages contain no Node.js runtime or Node backend. Version 1.7.0 adds releases for multiple desktop platforms and signed updates: WebView2 on Windows, system WebKit on macOS, and WebKitGTK 4.1 on Linux.

## Features

- Independent connections to multiple devices, with password, keyboard-interactive password, and private-key authentication.
- Local and remote port forwarding, with health checks and recovery.
- NVIDIA GPU monitoring, idle GPU counts, and scheduled refreshes.
- Personal and all-user Slurm job views, with Chinese state and reason labels.
- On-demand job priority queries and cancellation of your own jobs.
- Home-directory and temporary-file usage, plus cleanup of your own `/tmp` files.
- Light, dark, and system themes.
- System tray, connection logs, and credentials protected by system secure storage.
- Update checks at startup and every hour, with an update button beside the version number.

## How the features work

### Multiple SSH connections

Each device has independent asynchronous connection and monitoring tasks. A slow server does not block other devices or the window. The Rust backend uses `russh` and supports passwords, keyboard-interactive password authentication, private keys, and key passphrases. The first connection asks you to confirm the host fingerprint, and a changed fingerprint requires another confirmation. A new installation starts without preset devices.

### Port tunnels

Local forwarding connects a local listening port to a target reachable from the SSH server. Remote forwarding connects a server-side listening port to a target reachable from your computer. Rules default to loopback addresses, and each device can have multiple rules. The application checks the TCP forwarding path, closes a failed forwarding session, and retries with backoff. Rules remain saved when the target application is unavailable. A successful TCP check does not verify application protocols such as HTTP or SOCKS.

### GPU monitoring

After connecting, Lion SSH immediately queries NVIDIA GPUs with `nvidia-smi`. Refreshes run every 5 seconds by default, configurable from 2–300 seconds, with pause and manual refresh controls. The workspace shows idle counts, usage details, and a countdown ring. Idle status requires consecutive samples. A failed query preserves previous results and marks their status. Restricted parameterized queries fall back to ordinary `nvidia-smi` output. GPU monitoring works without Slurm; the card is hidden when no readable GPU is available.

### Slurm jobs

The application detects `squeue` and refreshes every 30 seconds. The personal view runs `squeue -u login_username`; the all-user view runs `squeue`. Job states and queue reasons have Chinese labels, with separate node and reason columns and original codes available on hover. Servers without Slurm do not show the job card.

### Job priority and cancellation

Each job row can request the Priority returned by `scontrol show job job_id`. Your own jobs can be cancelled with `scancel job_id` after confirmation. The backend checks the full job-owner information returned by the server before preparing confirmation and immediately before cancellation. A disconnected session or expired confirmation requires confirmation again. Failed cancellation is never retried automatically.

### Personal storage and temporary files

The storage card shows allocated disk space owned by the login user under `/home/username` and `/tmp`, together with a total. It scans immediately after connection and every 5 minutes. Ownership uses the SSH session's actual UID. Hard links are deduplicated, child symlinks are not followed, and filesystem boundaries are respected. Other users' inaccessible private directories are skipped; your files in accessible shared directories still count. The server needs GNU `find` and `awk`. Read failures remain visible rather than being reported as zero usage.

“Clean my /tmp” first previews the scope and asks for confirmation, then removes your own regular files, links, and removable empty directories. It skips other users' private directories, does not follow links, and does not delete their files. Cleanup is irreversible and can affect files currently in use. UID 0 accounts cannot use this feature, and failed cleanup is not retried automatically.

### Appearance and system tray

The glass-style interface offers light, dark, and system themes, with a matching title bar. Closing the window hides it to the system tray while connections and monitoring continue. Double-click the tray icon or choose the open command to restore the window. The tray's quit command closes tunnels and SSH connections before exiting. Tray support depends on the desktop environment.

### Logs and credentials

The workspace records connection, tunnel, and recovery events. Remembered passwords and private-key passphrases are protected by native secure storage; private-key contents are never copied into configuration files. If secure storage is unavailable, connect without remembering the credential. The frontend calls a restricted API with fixed command templates. Lion SSH does not expose an arbitrary remote-command terminal.

## Download and install

**[Download Lion SSH from GitHub Releases](https://github.com/ZjDing989/LionSSH/releases)**

Choose the attachment matching your system and processor. The release workflow targets the following 1.7.0 artifacts; available versions and actual build results are shown on Releases and [GitHub Actions](https://github.com/ZjDing989/LionSSH/actions/workflows/release.yml).

| Platform | File | Installation |
| --- | --- | --- |
| Windows x64, multi-file portable | `Lion-SSH-1.7.0-windows-x64-portable.zip` | Extract and run `Lion SSH.exe`; keep the complete folder and its adjacent `data/` |
| Windows x64, installer | `Lion-SSH-1.7.0-windows-x64-setup.exe` | Installs for the current user with a Start menu entry; defaults to `%LOCALAPPDATA%\Programs\Lion SSH` |
| macOS, Apple Silicon | `Lion-SSH-1.7.0-macos-aarch64.dmg` | Open the DMG, drag the application to Applications, then launch the installed app |
| macOS, Intel | `Lion-SSH-1.7.0-macos-x86_64.dmg` | Open the DMG, drag the application to Applications, then launch the installed app |
| Linux x86_64, AppImage | `Lion-SSH-1.7.0-linux-x86_64.AppImage` | Make it executable and run it; keep it in a user-writable directory for built-in updates |

The Windows installer lets you change its destination, and upgrades retain your selected installation directory. Both Windows editions read and write the `data/` folder beside the executable. Windows 10/11 and [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) are required. The installer checks for WebView2 and downloads it if missing; portable users need an existing runtime installation.

Linux needs a desktop environment with WebKitGTK 4.1. Remembering credentials also requires an unlocked desktop Secret Service, such as GNOME Keyring or a compatible KWallet service. You can connect without remembering passwords if no usable keyring is available. To launch an AppImage:

```sh
chmod +x Lion-SSH-1.7.0-linux-x86_64.AppImage
./Lion-SSH-1.7.0-linux-x86_64.AppImage
```

**Signing and operating-system trust:** Update packages use Tauri signatures verified with the application's embedded public key. The current macOS release workflow does not configure Apple Developer ID signing or Apple notarization. An updater signature does not establish macOS platform trust, so Gatekeeper may block the first launch. After verifying that the download came from this project's Releases, follow [Apple's guidance for safely opening applications](https://support.apple.com/en-us/102445). Windows Authenticode signing is also not configured, so SmartScreen prompts are possible. See [Tauri's macOS signing documentation](https://v2.tauri.app/distribute/sign/macos/) for release signing and notarization setup.

## Application updates

Lion SSH checks GitHub for the latest stable release at startup and every hour afterward. When a newer version is available, an update button appears beside the version number at the bottom of the sidebar. Checking does not install an update automatically.

For editions that support automatic installation, clicking update performs these steps:

1. Download the matching platform and edition, then verify its update signature and version.
2. Close tunnels and SSH connections after verification succeeds, before installing.
3. Restart the application after installation. A download or verification failure displays an error and leaves the running application usable.

| Edition | Update behavior |
| --- | --- |
| Windows installer | Updates with NSIS, preserving the current installation directory and `data/` |
| Windows multi-file portable | Updates distributed files in the same folder from a signed ZIP; preserves `data/` and extra user files, does not run NSIS or convert to an installed edition; keep `portable.json` for portable-update detection |
| macOS application | Updates the application bundle from the matching signed `.app.tar.gz`, then restarts |
| Linux AppImage | Updates from a signed AppImage; the application location must be writable |
| Linux source or other non-AppImage builds | Shows a download link to Releases, where you can obtain the official AppImage |

The `.sig` files and `.app.tar.gz` archives on Releases primarily serve the updater. For an initial installation, normally choose an EXE, ZIP, DMG, or AppImage from the download table.

## Run from source

Development requires Node.js **22.12+**, npm, and stable Rust **1.90+**. Node.js is used only for frontend builds, packaging scripts, and development tests. It is not required to run a release package.

| Platform | Native development dependencies |
| --- | --- |
| Windows | MSVC Rust toolchain, Visual Studio C++ Build Tools with Desktop development with C++ and a Windows SDK, and WebView2 |
| macOS | Xcode or Xcode Command Line Tools, plus the Rust target for the selected architecture |
| Linux | A C/C++ build toolchain, WebKitGTK 4.1, and Tauri's GTK, tray, SVG, and other development libraries |

Follow the [official Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform. The first native build downloads the Rust dependencies pinned in Cargo.lock; the first Windows bundle build also downloads NSIS tools.

```sh
npm ci
npm start
```

`npm start` and `npm run dev` launch Vite and the Tauri desktop application. The frontend development server uses `http://127.0.0.1:1420`. Project scripts also look for Cargo in the user's standard toolchain directory.

```sh
npm run build       # TypeScript checks and frontend build
npm run test:ui     # Browser interaction tests; requires installed Chrome
npm run tauri -- info
```

### Tests

```powershell
# The complete suite currently targets Windows
npm.cmd test
```

`npm test` runs Rust unit tests, compiles the native backend, and exercises it against simulated loopback SSH servers. The development-only `ssh2` package implements the test servers; the real client is always Rust. On macOS/Linux, install the native prerequisites and run Rust tests. Prepare frontend and license resources before the first run:

```sh
npm run build:frontend
npm run prepare:licenses
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

Envelope and path unit tests use injected fixture data and do not require unlocking a real keyring in CI. Windows desktop checks use the generated portable package:

```powershell
npm.cmd run smoke
npm.cmd run smoke:data
```

`smoke` checks real WebView2 communication, simulated SSH connections, bidirectional tunnels, and single-instance restoration with isolated data and a separate test-instance namespace. It does not operate an existing user instance. `smoke:data` launches an isolated installation layout without a portable marker from a different working directory, checking adjacent legacy-format `data/` and the `LION_DATA_DIR` override. UI tests require installed Chrome. Native packages and operating-system integration should also be exercised on their target systems.

## Local packaging

The project generates signed updater artifacts by default. Before a release build, supply `TAURI_SIGNING_PRIVATE_KEY` matching the embedded public key and its password when needed. GitHub Actions reads these values from repository Secrets. Development with `npm start` does not require a release private key. See the [Tauri updater documentation](https://v2.tauri.app/plugin/updater/) for signing setup.

### Windows

From the project directory in Windows PowerShell:

```powershell
npm.cmd ci
npm.cmd run dist:win
```

One build creates an installer and a multi-file portable edition under `release/`:

```text
release/
├─ Lion-SSH-1.7.0-windows-x64-portable/
│  ├─ Lion SSH.exe
│  ├─ portable.json
│  ├─ licenses/            # Third-party notices and licenses
│  ├─ LICENSE.txt
│  ├─ 使用说明.txt
│  └─ manifest.json        # SHA256 manifest of distributed files
├─ Lion-SSH-1.7.0-windows-x64-portable.zip
└─ Lion-SSH-1.7.0-windows-x64-setup.exe
```

`npm run pack` creates only the portable folder and ZIP. `npm run dist` is equivalent to `npm run dist:win`. Repackaging removes old distributed files using the previous manifest while preserving existing `data/` and extra user files. The ZIP contains only this build's distribution files. `scripts/licenses.cjs` generates license resources from locked Rust and frontend production dependencies for both editions.

If the corresponding target has already been built, reuse its output to create and sign the portable package. Supply signing credentials through the environment:

```powershell
node scripts/package.cjs --skip-build --target x86_64-pc-windows-msvc --sign-portable
```

`TAURI_SIGNING_PRIVATE_KEY` supplies private-key contents. To have the local signer read a file instead, use `TAURI_SIGNING_PRIVATE_KEY_PATH`; provide the key password through `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Do not put actual key contents or passwords in packaging scripts.

### macOS and Linux

Use the Tauri CLI on the target operating system after installing the appropriate Rust target. The two macOS commands build Apple Silicon and Intel editions respectively:

```sh
npm run tauri -- build --target aarch64-apple-darwin --bundles app,dmg
npm run tauri -- build --target x86_64-apple-darwin --bundles app,dmg
```

Linux x86_64:

```sh
npm run tauri -- build --target x86_64-unknown-linux-gnu --bundles appimage
```

Tauri writes native bundles to `src-tauri/target/<target>/release/bundle/`. Configure signing credentials to generate official updater packages. The GitHub workflow applies consistent release asset names and uploads them. An ordinary local build does not create a GitHub Release.

## GitHub Actions releases

[`.github/workflows/release.yml`](.github/workflows/release.yml) runs on numeric `v*` version tags and supports manual dispatch. Manual runs require an existing tag such as `v1.7.0`. The tag version must match `package.json`, `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`.

Set these names under repository **Settings → Secrets and variables → Actions**:

| Secret | Purpose |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | Required Tauri updater signing private key |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Required when the private key is password protected |

The update public key is already configured in the application. Never put actual private-key or password values in README files, source code, commits, or workflow logs. The local `.secrets/` directory contains encrypted backups and is ignored by Git; it is not committed or included in release packages.

The workflow first creates or reuses a **draft Release** for the tag, then builds Windows x64, macOS Apple Silicon, macOS Intel, and Linux x86_64. It generates `latest.json` and publishes only after every build succeeds and final checks confirm that required assets and updater signatures are present and bound to the matching version. If a platform fails, the release remains a draft for investigation and is not promoted to the latest stable release.

`latest.json` supplies update entries for Windows installed and portable editions, both macOS architectures, and Linux AppImage. A typical release uses:

```sh
git push origin main
git tag v1.7.0
git push origin v1.7.0
```

Pushing a tag starts the workflow. Check that run and its Releases attachments to establish whether building, signing, and publication succeeded.

## Data and upgrades

| Platform | Default data directory |
| --- | --- |
| Windows source development | The project's `data/` |
| Windows installed / portable | `data/` beside the application `.exe`, independent of the launch working directory |
| macOS | `~/Library/Application Support/tools.lion.ssh/` |
| Linux | `$XDG_DATA_HOME/tools.lion.ssh/`, or `~/.local/share/tools.lion.ssh/` when no valid absolute XDG path is set |

`LION_DATA_DIR` overrides the default with another writable directory; an absolute path is recommended. Windows loads adjacent `data/` without a `portable.json` marker. That marker selects the portable update mechanism. Windows WebView2 cache is stored under `webview/` in the selected data directory.

Configuration uses `profiles.json` for devices and tunnels, `settings.json` for the theme, `known-hosts.json` for trusted fingerprints, and `credentials.json` for encrypted secrets. The Rust backend preserves these JSON formats and unknown extension fields, and does not rewrite existing configuration at startup. Before a Windows upgrade, exit the application and back up the old `data/`; preserve the folder when upgrading. If you used an early Tauri installer with `%APPDATA%/tools.lion.ssh/`, exit the application and copy its configuration into the current installation's `data/`, backing up matching files before merging.

Windows retains DPAPI and the existing `native:v1:` format. Credentials saved by previous Tauri versions remain usable under the same Windows account. macOS Keychain and Linux desktop Secret Service protect an encryption master key, while credentials in configuration files use AES-256-GCM encryption. Credentials are bound to the original system account and secure store. Copying configuration to another operating system or account does not make saved passwords decryptable; enter and save them again. Earlier, different ciphertext formats also require re-saving secrets, while device, tunnel, theme, and host-fingerprint configuration remains readable.

macOS/Linux store configuration in the user directories above, independently of read-only application bundles or AppImage mounts. Automatic updates still need permission to replace the installed application files. If the system keyring is locked or unavailable, the application never falls back to plaintext storage; connect without remembering the password instead. Keep personal data, credentials, runtime logs, test output, and private signing keys out of the repository.

## Project layout

```text
src/                      Vue UI, types, and Tauri communication adapter
src-tauri/src/            Rust windows, tray, SSH, tunnels, monitoring, data, crypto, updates
src-tauri/windows/        NSIS installer template
src-tauri/                Tauri configuration, capabilities, and locked Rust dependencies
scripts/                  Development, packaging, release, license, and UI check scripts
tests/                    Packaging, release metadata, and loopback SSH tests
.github/workflows/        Multi-platform builds and draft-release publication
assets/                   Application icons
```

Generated `dist/`, `release/`, `src-tauri/target/`, `src-tauri/resources/licenses/`, `test-output/`, and `.secrets/` are not committed.

## License

Licensed under [Apache License 2.0](LICENSE). Third-party dependencies retain their respective licenses and copyright notices.
