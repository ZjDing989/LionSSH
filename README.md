<p align="center"><strong>简体中文</strong> · <a href="README.en.md">English</a></p>

<h1 align="center">Lion SSH | <img src="assets/icon.png" alt="Lion SSH Logo" width="40" height="40" /></h1>

<p align="center">多设备 SSH、端口隧道、显卡监测与 Slurm 作业管理，在一个简洁的工作台内完成。</p>

桌面应用基于 **Tauri 2 + Rust + Vue 3 + TypeScript**。SSH、隧道、监测、配置存储和凭据加密均在 Rust 主程序内运行，发行包不包含 Node.js 运行时或 Node 后台。1.7.1 加入跨平台发布与签名更新：Windows 使用 WebView2，macOS 使用系统 WebKit，Linux 使用 WebKitGTK 4.1。

## 功能

- 多设备独立连接，支持密码、交互式密码和密钥认证。
- 正向 / 反向端口隧道，自动检测与恢复。
- NVIDIA 显卡监测、空闲数量与定时刷新。
- Slurm 个人 / 全部作业查看，中文状态与原因。
- 作业权重查询及本人作业取消。
- 个人目录、临时文件占用统计与本人 `/tmp` 清理。
- 白天、夜晚、跟随系统三种主题。
- 托盘常驻、连接日志与系统加密保存凭据。
- 启动时及每小时检查新版本，在版本号旁显示更新入口。

## 功能介绍

### 多设备 SSH

每台设备使用独立的异步连接与监测任务，某台服务器响应缓慢不会阻塞其他设备或窗口。Rust 后台通过 `russh` 实现 SSH，支持密码、交互式密码认证、私钥和私钥口令；首次连接确认主机指纹，指纹变化时重新确认。首次启动没有预置设备。

### 端口隧道

正向隧道把本机端口连接到服务器侧目标，反向隧道把服务器端口连接到本机侧目标。默认监听回环地址，支持多条规则。程序检查 TCP 转发路径，异常时先关闭旧转发，再退避重试；目标应用不可用时保留规则。TCP 连通结果不代表已验证 HTTP、SOCKS 等应用协议。

### 显卡监测

连接后立即使用 `nvidia-smi` 检测 NVIDIA 显卡，默认每 5 秒刷新，可调整为 2–300 秒，支持暂停和手动刷新。显示空闲张数、使用详情与倒计时圆环；连续采样确认空闲，读取失败时保留上次结果并标记状态。参数化查询受限时尝试兼容的 `nvidia-smi` 状态输出。监测不依赖 Slurm，没有可读取显卡时自动收起卡片。

### Slurm 作业查看

自动识别可用的 `squeue`，默认每 30 秒更新。“个人”执行 `squeue -u 登录用户名`，“全部”执行 `squeue`。作业状态和排队原因显示中文，节点列表与原因分列，悬停可查看原始代号；未安装 Slurm 的服务器不展示作业卡片。

### 作业权重与取消

每行可按需查询 `scontrol show job 作业编号` 返回的 Priority。自己的作业支持确认后执行 `scancel 作业编号`；确认前后均核对服务器返回的完整用户归属，断线或确认过期后需重新确认，失败不会自动重试取消操作。

### 个人空间与临时文件

显示登录用户在 `/home/用户名`、`/tmp` 中拥有的已分配磁盘空间及合计，连接后立即统计，每 5 分钟更新。使用 SSH 会话实际 UID 筛选归属，硬链接去重，不跟随子目录链接、不跨文件系统；跳过其他用户不可访问的私有目录，共享目录内属于本人的文件仍会计入。需要服务器提供 GNU `find` 和 `awk`，本人目录确实读取失败时保留提示。

“清理我的 /tmp”先预览范围并确认，再删除当前用户自己的普通文件、链接及可移除的空目录。清理同样跳过他人的私有目录，不跟随链接，不删除他人的文件。操作不可撤销，正在使用的文件也可能受影响；UID 为 0 的账号禁用此功能，失败不自动重试。

### 外观与托盘

简约玻璃风格界面提供白天、夜晚和跟随系统模式，标题栏与内容保持一致。关闭窗口会隐藏到系统托盘，连接和监测继续运行；双击图标或选择“打开软件”恢复窗口，托盘菜单的“关闭软件”会先关闭隧道及 SSH 连接，再退出。托盘可用性取决于桌面环境。

### 系统日志与凭据

工作台记录连接、隧道和恢复事件。勾选“记住凭据”后通过系统安全存储保护密码或密钥口令，私钥内容不写入配置；安全存储不可用时仍可使用不记住密码的会话。前端通过限定 API 调用固定命令模板，应用不提供任意远程命令终端。

## 下载与安装

**[前往 GitHub Releases 下载 Lion SSH](https://github.com/ZjDing989/LionSSH/releases)**

选择与系统和处理器匹配的附件。以 1.7.1 为例，发布流程的目标产物如下；实际可下载版本及构建结果以 Releases 和 [GitHub Actions](https://github.com/ZjDing989/LionSSH/actions/workflows/release.yml) 为准。

| 平台 | 文件 | 使用方式 |
| --- | --- | --- |
| Windows x64 多文件便携版 | `Lion-SSH-1.7.1-windows-x64-portable.zip` | 解压后运行 `Lion SSH.exe`，保留整个目录和旁边的 `data/` |
| Windows x64 安装版 | `Lion-SSH-1.7.1-windows-x64-setup.exe` | 当前用户安装，创建开始菜单入口；默认安装目录为 `%LOCALAPPDATA%\Programs\Lion SSH` |
| macOS Apple Silicon | `Lion-SSH-1.7.1-macos-aarch64.dmg` | 适用于 Apple 芯片；打开 DMG，将应用拖到“应用程序”后运行 |
| macOS Intel | `Lion-SSH-1.7.1-macos-x86_64.dmg` | 适用于 Intel 芯片；打开 DMG，将应用拖到“应用程序”后运行 |
| Linux x86_64 AppImage | `Lion-SSH-1.7.1-linux-x86_64.AppImage` | 添加可执行权限后运行，放在当前用户可写目录可使用内置更新 |

Windows 安装时可以修改目录，升级会保留已经选择的安装目录。安装版和便携版都在主程序旁读取及保存 `data/`。两种版本支持 Windows 10/11，均需要 [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)；安装版会检测并在缺少时联网安装 WebView2，便携版需要电脑已安装该运行时。

Linux 需要提供 WebKitGTK 4.1 的桌面环境。记住凭据还需要已解锁的桌面 Secret Service，例如 GNOME Keyring 或兼容的 KWallet；没有可用密钥环时可以不记住密码。AppImage 示例：

```sh
chmod +x Lion-SSH-1.7.1-linux-x86_64.AppImage
./Lion-SSH-1.7.1-linux-x86_64.AppImage
```

**签名与系统信任：** 自动更新包使用应用内公钥验证的 Tauri 更新签名。当前 macOS 发布流程尚未配置 Apple Developer ID 代码签名和 Apple 公证，因此更新签名不等于 macOS 的系统信任，Gatekeeper 仍可能阻止首次打开。核实下载来自本项目 Releases 后，可参照 [Apple 的应用安全打开说明](https://support.apple.com/zh-cn/102445)。Windows 当前也未配置 Authenticode 代码签名，可能出现 SmartScreen 提示。macOS 正式代码签名与公证的配置方式见 [Tauri 文档](https://v2.tauri.app/distribute/sign/macos/)。

## 软件更新

应用启动时检查 GitHub 最新正式版，此后每小时检查一次。发现高于当前版本的新版本后，侧边栏底部版本号旁会出现“更新”按钮；检查不会自动安装。

点击更新后，支持自动安装的版本按以下顺序处理：

1. 下载对应平台及发行形式的更新包，并校验更新签名和版本。
2. 校验成功后关闭隧道及 SSH 连接，再开始安装。
3. 安装完成后重新启动应用。安装前下载或校验失败会显示错误，当前应用仍可继续使用。

| 发行形式 | 更新方式 |
| --- | --- |
| Windows 安装版 | 使用 NSIS 安装包更新，继续使用当前安装目录和 `data/` |
| Windows 多文件便携版 | 使用签名 ZIP 在原目录更新分发文件，保留 `data/` 和用户额外文件，不调用 NSIS、不转换成安装版；请保留 `portable.json` 以识别便携更新模式 |
| macOS 应用 | 使用相应架构的签名 `.app.tar.gz` 更新应用包，然后重启 |
| Linux AppImage | 使用签名 AppImage 更新；所在目录需要可写 |
| Linux 源码或其他非 AppImage 构建 | 显示“下载新版本”入口，前往 Releases 获取正式 AppImage |

发布页中的 `.sig` 和 `.app.tar.gz` 主要供更新流程使用，首次安装通常选择上方表格中的 EXE、ZIP、DMG 或 AppImage。

## 从源码运行

开发环境需要 Node.js **22.12+**、npm，以及 Rust **1.90+** stable。Node.js 只用于前端构建、打包脚本和开发测试，软件运行时不需要它。

| 平台 | 原生开发依赖 |
| --- | --- |
| Windows | MSVC Rust 工具链、Visual Studio C++ Build Tools 的“使用 C++ 的桌面开发”和 Windows SDK、WebView2 |
| macOS | Xcode 或 Xcode Command Line Tools，以及所构建架构的 Rust target |
| Linux | 系统 C/C++ 构建工具、WebKitGTK 4.1 和 Tauri 所需的 GTK、托盘、SVG 等开发库 |

按所在系统完成 [Tauri 官方前置要求](https://v2.tauri.app/zh-cn/start/prerequisites/)。首次原生构建会下载 Cargo 锁文件指定的 Rust 依赖，Windows 首次打包还会下载 NSIS 工具。

```sh
npm ci
npm start
```

`npm start` 与 `npm run dev` 都会启动 Vite 和 Tauri 桌面应用，前端开发服务固定为 `http://127.0.0.1:1420`。项目脚本会自动查找用户目录下的 Cargo 工具链。

```sh
npm run build       # TypeScript 类型检查与前端构建
npm run test:ui     # 浏览器界面交互测试，需要已安装 Chrome
npm run tauri -- info
```

### 测试

```powershell
# 完整套件目前面向 Windows
npm.cmd test
```

`npm test` 包括 Rust 单元测试、原生后台编译和本机模拟 SSH 集成测试；模拟服务器来自仅供开发的 `ssh2`，实际客户端为 Rust 后台。macOS/Linux 可在安装原生前置依赖后运行 Rust 测试；首次执行前准备前端和许可证资源：

```sh
npm run build:frontend
npm run prepare:licenses
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

密文信封和路径测试使用注入的测试数据，不要求 CI 解锁真实系统密钥环。Windows 的真实桌面验证使用生成的便携版：

```powershell
npm.cmd run smoke
npm.cmd run smoke:data
```

`smoke` 在隔离数据目录和独立测试实例命名空间中，验证 WebView2 桌面通信、模拟 SSH、双向隧道和单实例恢复，不操作已打开的用户实例。`smoke:data` 使用没有便携标识的隔离安装布局，从不同工作目录验证程序旁的旧格式 `data/` 和 `LION_DATA_DIR` 优先级。界面测试需要已安装 Chrome；对应平台的实际打包和系统集成还应通过目标系统上的运行验证。

## 本地打包

项目默认生成用于更新的签名产物。正式打包前应通过环境提供与应用公钥配对的 `TAURI_SIGNING_PRIVATE_KEY` 和所需密码；GitHub Actions 从仓库 Secrets 读取这些值。开发运行 `npm start` 不需要发布私钥。签名配置说明见 [Tauri 更新文档](https://v2.tauri.app/plugin/updater/)。

### Windows

在 Windows PowerShell 的项目目录运行：

```powershell
npm.cmd ci
npm.cmd run dist:win
```

一次构建同时生成安装版和多文件便携版，输出在 `release/`：

```text
release/
├─ Lion-SSH-1.7.1-windows-x64-portable/
│  ├─ Lion SSH.exe
│  ├─ portable.json
│  ├─ licenses/            # 第三方开源声明与许可证
│  ├─ LICENSE.txt
│  ├─ 使用说明.txt
│  └─ manifest.json        # 分发文件 SHA256 清单
├─ Lion-SSH-1.7.1-windows-x64-portable.zip
└─ Lion-SSH-1.7.1-windows-x64-setup.exe
```

`npm run pack` 只生成多文件便携目录及 ZIP，`npm run dist` 与 `npm run dist:win` 等效。重新打包时按旧清单移除旧程序文件，保留已有 `data/` 和用户额外文件；ZIP 只包含本次分发文件。`scripts/licenses.cjs` 根据锁定的 Rust 和前端生产依赖生成许可证资源，并随两个版本分发。

已完成对应 target 的原生构建时，可复用产物生成并签名便携包；签名密钥必须通过环境提供：

```powershell
node scripts/package.cjs --skip-build --target x86_64-pc-windows-msvc --sign-portable
```

`TAURI_SIGNING_PRIVATE_KEY` 传私钥内容；本地 signer 从文件读取时使用 `TAURI_SIGNING_PRIVATE_KEY_PATH`，密钥密码由 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 提供。不要把实际内容或密码写进打包脚本。

### macOS 与 Linux

在目标系统上安装相应 Rust target 后使用 Tauri CLI。macOS 两条命令分别生成 Apple Silicon 和 Intel 版本：

```sh
npm run tauri -- build --target aarch64-apple-darwin --bundles app,dmg
npm run tauri -- build --target x86_64-apple-darwin --bundles app,dmg
```

Linux x86_64：

```sh
npm run tauri -- build --target x86_64-unknown-linux-gnu --bundles appimage
```

Tauri 原生产物位于 `src-tauri/target/<target>/release/bundle/`。生成用于自动更新的正式包需要配置签名密钥；GitHub 流程会统一命名并上传发布附件。本地普通构建不会自动创建 GitHub Release。

## GitHub Actions 发布

[`.github/workflows/release.yml`](.github/workflows/release.yml) 支持推送 `v*` 数字版本标签和手动触发。手动运行需填写已经存在的标签，例如 `v1.7.1`；标签版本必须与 `package.json`、`src-tauri/Cargo.toml` 和 `src-tauri/tauri.conf.json` 一致。

仓库的 **Settings → Secrets and variables → Actions** 使用以下名称：

| Secret | 用途 |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | Tauri 更新签名私钥，必须配置 |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | 私钥的解密密码；私钥设有密码时必须配置 |

更新公钥已在应用配置中设置。私钥和密码的实际值不得写入 README、代码、提交记录或工作流日志；本地 `.secrets/` 保存加密备份并已被 Git 忽略，不随源码提交，也不进入发行包。

发布流程先创建或复用该标签的 **draft Release**，随后构建 Windows x64、macOS Apple Silicon、macOS Intel 和 Linux x86_64。只有全部构建成功，且最终检查确认所需附件及更新签名齐全、签名绑定的版本一致后，才生成 `latest.json` 并公开发布。某个平台失败时保留草稿供排查，不会把不完整版本设为最新正式版。

`latest.json` 包含 Windows 安装版与便携版、两种 macOS 架构和 Linux AppImage 的更新入口。常规发布操作为：

```sh
git push origin main
git tag v1.7.1
git push origin v1.7.1
```

推送标签会启动工作流；是否构建、签名和发布成功，应查看该次 Actions 运行及 Releases 附件。

## 数据与升级

| 平台 | 默认数据目录 |
| --- | --- |
| Windows 源码开发 | 项目的 `data/` |
| Windows 安装版 / 便携版 | 主程序 `.exe` 所在目录下的 `data/`，与启动工作目录无关 |
| macOS | `~/Library/Application Support/tools.lion.ssh/` |
| Linux | `$XDG_DATA_HOME/tools.lion.ssh/`；未设置有效绝对 XDG 路径时使用 `~/.local/share/tools.lion.ssh/` |

`LION_DATA_DIR` 优先指定其他可写目录，建议使用绝对路径。Windows 读取旁边的 `data/` 不依赖 `portable.json`；该文件用于识别便携版更新方式。Windows 的 WebView2 缓存保存在所选数据目录的 `webview/` 下。

配置包含 `profiles.json`（设备及隧道）、`settings.json`（主题）、`known-hosts.json`（主机指纹）及 `credentials.json`（加密凭据）。Rust 后台保留这些 JSON 格式及未知扩展字段，启动不会重写已有配置。Windows 升级前退出程序并备份旧 `data/`，升级时保留该目录；若使用过早期 Tauri 安装版的 `%APPDATA%/tools.lion.ssh/`，需先退出程序，再把其中配置复制到当前安装目录的 `data/`，合并同名文件前先备份。

Windows 凭据继续使用当前用户的 DPAPI 和原有 `native:v1:` 格式，同一 Windows 账户下的现有 Tauri 凭据可以继续使用。macOS 使用系统 Keychain，Linux 使用桌面 Secret Service 保护加密主密钥，配置中的凭据以 AES-256-GCM 加密保存。凭据绑定保存时的系统账户和安全存储；复制配置到另一操作系统或账户，不代表已保存的密码可以解密，需要重新输入并保存。更早版本的其他密文格式同样需要重新保存，设备、隧道、主题和主机指纹仍可读取。

macOS/Linux 的配置保存在上述用户目录中，不依赖应用包或 AppImage 挂载目录可写；自动更新仍需要能够替换已安装的应用文件。系统密钥环不可用或被锁定时，应用不会把密码改存为明文，可选择不记住密码。个人数据、凭据、运行日志、测试输出和签名私钥不应提交到仓库。

## 项目结构

```text
src/                      Vue 界面、类型与 Tauri 通信适配
src-tauri/src/            Rust 窗口、托盘、SSH、隧道、监测、数据、加密与更新
src-tauri/windows/        NSIS 安装模板
src-tauri/                Tauri 配置、能力权限、Rust 依赖锁文件
scripts/                  开发、打包、发布、许可证与界面验证脚本
tests/                    打包、发布元数据及本机模拟 SSH 测试
.github/workflows/        多平台构建与草稿发布流程
assets/                   应用图标
```

`dist/`、`release/`、`src-tauri/target/`、`src-tauri/resources/licenses/`、`test-output/` 和 `.secrets/` 均不提交到仓库。

## 开源协议

本项目使用 [Apache License 2.0](LICENSE)。第三方依赖保留各自的开源协议与版权声明。
