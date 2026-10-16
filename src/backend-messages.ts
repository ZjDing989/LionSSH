// Translate only app-authored presentation fields. Never pass terminal output,
// user remarks, server names, paths, or raw command output through this helper.
const exact = new Map<string, string>(`
SSH 连接已建立|SSH connection established
SSH 连接已关闭|SSH connection closed
SSH 连接已断开|SSH connection disconnected
SSH 连接中断|SSH connection interrupted
SSH 连接超时|SSH connection timed out
SSH 连接或认证超时|SSH connection or authentication timed out
SSH 认证超时|SSH authentication timed out
SSH 认证失败，请检查用户名和登录凭据|SSH authentication failed; check the username and credentials
SSH 交互认证次数超过限制|Too many SSH authentication attempts
SSH 私钥文件格式不正确|Invalid SSH private key format
SSH 通道建立超时|SSH channel creation timed out
SSH 未连接|SSH is not connected
连接已关闭|Connection closed
连接配置不完整|Connection settings are incomplete
服务器关闭了 SSH 连接|The server closed the SSH connection
服务器要求多个交互认证项，当前连接仅支持单密码认证|The server requires multiple authentication responses; only one password is supported
已拒绝服务器指纹|Server fingerprint rejected
服务器指纹确认超时或连接已取消，请重新连接|Fingerprint confirmation timed out or was cancelled; reconnect to continue
主机指纹请求已过期|The server fingerprint request has expired
认证方式无效|Invalid authentication method
认证方式不正确|Invalid authentication method
请选择 SSH 私钥文件|Select an SSH private key file
请输入 SSH 密码|Enter the SSH password
请先输入 SSH 密码|Enter the SSH password first
私钥文件过大|The private key file is too large
密钥文件不存在或无法读取，请检查路径|The key file does not exist or cannot be read; check its path
请先连接设备|Connect to the server first
配置未就绪|Settings are not ready
设备不存在|Server not found
设备配置不能为空|Server settings cannot be empty
不支持的应用操作|Unsupported application operation
不支持的终端操作|Unsupported terminal operation
不支持的远程操作|Unsupported remote operation
应用正在关闭|The application is closing
Lion SSH 启动失败|Lion SSH could not start
主窗口配置缺失|The main window configuration is missing
无法初始化异步运行时|Cannot initialize the asynchronous runtime
正在关闭隧道和服务器连接…|Closing tunnels and SSH connections…
调用参数格式不正确|Invalid request arguments
请求格式无效|Invalid request format
请求内容过长|The request is too large
不允许的调用来源|The request source is not allowed
更新操作不接受参数|Update operations do not accept arguments
选择密钥的请求格式无效|Invalid key selection request
系统主题格式不正确|Invalid system theme
请选择白天、夜晚或跟随系统|Choose Light, Dark, or Follow system
请选择中文、English 或跟随系统|Choose Chinese, English, or Follow system
凭据格式不正确|Invalid credential format
凭据内容过长|The credentials are too long
凭据过长|The credentials are too long
保存的凭据格式无效|Invalid saved credential format
解密后的凭据格式无效|Invalid decrypted credential format
系统加密未返回有效凭据|System encryption did not return valid credentials
无法解密保存的凭据，请重新输入密码或密钥口令|Cannot decrypt the saved credentials; enter the password or key passphrase again
此凭据属于其他操作系统，请在当前系统重新输入并保存密码或密钥口令|These credentials belong to another operating system; enter and save them again on this system
Windows 无法安全保存凭据|Windows could not securely save the credentials
Windows 无法解密凭据，请使用保存时的 Windows 账户，或重新输入凭据|Windows could not decrypt the credentials; use the original Windows account or enter them again
此平台不支持系统安全凭据存储；可以使用不记住密码的会话|Secure credential storage is unavailable; continue without remembering the password
此平台不支持系统安全凭据存储，请重新输入密码或密钥口令|Secure credential storage is unavailable; enter the password or key passphrase again
系统凭据密钥格式无效|Invalid system credential key format
系统安全随机数不可用，无法安全保存凭据|Secure random numbers are unavailable; credentials cannot be saved securely
系统无法安全保存凭据|The system could not securely save the credentials
系统无法解密凭据，请使用保存时的系统账户及密钥环，或重新输入凭据|Cannot decrypt the credentials; use the original system account and keyring, or enter them again
系统密钥环未保存凭据密钥，请解锁密钥环后重试|The credential key was not saved in the keyring; unlock it and retry
系统密钥环中未找到凭据密钥，请使用保存时的系统账户，或重新输入凭据|The credential key is missing from the keyring; use the original account or enter the credentials again
系统密钥环中的凭据密钥格式无效，原密钥不会被自动覆盖|The credential key in the keyring is invalid; the original key will be preserved
系统密钥环不可用或已锁定，无法记住密码；请解锁 macOS 钥匙串或 Linux Secret Service，或使用不记住密码的会话|The keyring is unavailable or locked; unlock macOS Keychain or Linux Secret Service, or continue without remembering the password
无法确定系统凭据目录|Cannot locate the system credential directory
无法创建系统凭据锁目录|Cannot create the credential lock directory
无法打开系统凭据锁|Cannot open the credential lock
无法锁定系统凭据存储|Cannot lock the credential store
不支持的原生凭据操作|Unsupported native credential operation
无法确定应用程序目录|Cannot locate the application directory
无法确定用户数据目录，请通过 LION_DATA_DIR 指定可写目录|Cannot locate the user data directory; set LION_DATA_DIR to a writable directory
服务器地址格式不正确|Invalid server address
SSH 端口格式不正确|Invalid SSH port
每台设备最多支持 100 条隧道|Each server supports up to 100 tunnels
请选择正向或反向隧道|Choose a local or reverse tunnel
隧道配置格式不正确|Invalid tunnel settings
隧道标识重复|Duplicate tunnel ID
隧道 ID 重复|Duplicate tunnel ID
隧道缺少 ID|A tunnel ID is required
无效的隧道方向|Invalid tunnel direction
隧道不存在|Tunnel not found
请先启用隧道|Enable the tunnel first
本机目标连接超时|The local target connection timed out
本机端口监听超时|The local port listener timed out
转发通道连接超时|The forwarding channel connection timed out
远端监听请求超时|The remote listener request timed out
远端监听请求超时，将关闭 SSH 会话后恢复|The remote listener request timed out; the SSH session will close before recovery
取消远端监听超时|Stopping the remote listener timed out
旧远端监听未确认释放，将关闭 SSH 会话后恢复|The previous remote listener has not been released; the SSH session will close before recovery
等待旧远端监听释放|Waiting for the previous remote listener to be released
监听已建立，等待连通性检测|Listener established; waiting for a connectivity check
TCP 检测通过；未检查应用响应|TCP check passed; application responses were not checked
TCP 检测通过；反向转发路径及本机目标已验证；未检查应用响应|TCP check passed; the reverse tunnel and local target were verified; application responses were not checked
反向转发路径未返回确认，通道已关闭|The reverse forwarding path returned no confirmation and the channel closed
反向转发路径响应不匹配|The reverse forwarding path returned an unexpected response
反向转发路径未返回确认，检测超时|The reverse forwarding check timed out without confirmation
服务器限制远端探测|The server restricts remote probes
远端探测未验证|The remote probe was not verified
远程指令输出超过限制|Remote command output exceeded the size limit
远程指令执行超时|The remote command timed out
当前 SSH 执行器不支持文件输入|The current SSH executor does not support file input
读取上传文件超时|Reading the upload file timed out
上传文件长度与预期不符|The upload file length did not match the expected length
上传文件在传输过程中缩短|The upload file became shorter during transfer
上传文件在传输过程中变长|The upload file became longer during transfer
文件传输完成前远程指令被信号终止|The remote command was terminated by a signal before the transfer completed
文件传输完成前远程通道已结束输出|The remote channel ended its output before the transfer completed
文件传输完成前远程通道已关闭|The remote channel closed before the transfer completed
服务器拒绝远程指令|The server rejected the remote command
终端读取位置无效|Invalid terminal read position
终端已结束，请新建终端|The terminal has ended; open a new terminal
终端已关闭|Terminal closed
终端已结束|Terminal exited
终端不存在或已关闭|The terminal does not exist or has closed
终端通道仍在建立，请稍后重试|The terminal channel is still opening; retry shortly
终端通道建立超时|Opening the terminal channel timed out
SSH 终端建立超时|Opening the SSH terminal timed out
SSH 终端通道已关闭|The SSH terminal channel closed
终端建立已取消|Opening the terminal was cancelled
最多同时保留 64 个终端，请先关闭不用的终端|Up to 64 terminals can be retained; close unused terminals first
每台设备最多保留 16 个终端，请先关闭不用的终端|Up to 16 terminals can be retained per server; close unused terminals first
单次终端输入不能超过 64 KiB|A single terminal input cannot exceed 64 KiB
终端输入格式无效|Invalid terminal input format
终端备注需为 1 到 80 个字符，且不能包含控制字符|A terminal remark must contain 1–80 characters and no control characters
终端尺寸无效（列 2–500，行 1–300）|Invalid terminal dimensions (2–500 columns, 1–300 rows)
终端输入发送超时|Sending terminal input timed out
终端尺寸格式不正确|Invalid terminal dimensions
终端操作参数格式不正确|Invalid terminal operation arguments
终端读取位置格式不正确|Invalid terminal read position
服务器拒绝PTY 终端|The server rejected the PTY terminal
服务器拒绝交互式 Shell|The server rejected the interactive shell
建立PTY 终端时服务器关闭了通道|The server closed the channel while opening the PTY terminal
建立交互式 Shell时服务器关闭了通道|The server closed the channel while opening the interactive shell
SSH 未连接或 Slurm 操作已禁用|SSH is disconnected or Slurm operations are disabled
SSH 未连接或空间操作已禁用|SSH is disconnected or storage operations are disabled
SSH 未连接或连接已变化，请重新确认|SSH is disconnected or the connection changed; confirm again
Slurm 作业查询已启用|Slurm job monitoring enabled
当前连接已禁用 Slurm 作业查询|Slurm job monitoring is disabled for this connection
未找到 squeue，Slurm 作业监测不可用|squeue was not found; Slurm job monitoring is unavailable
作业查询范围不正确|Invalid job query scope
作业查询用户名不正确|Invalid job query username
作业范围应为个人或全部|Choose My jobs or All jobs
作业 ID 格式不正确|Invalid job ID
作业 ID 格式不正确，请刷新作业列表|Invalid job ID; refresh the job list
作业不存在，或服务器没有返回可验证的作业详情|The job does not exist or the server returned no verifiable job details
作业列表已过期，请先刷新|The job list is outdated; refresh it first
作业所属用户信息不一致|The job owner information does not match
作业所属用户已变化，请重新确认|The job owner changed; confirm again
作业数组范围不正确|Invalid job array range
作业权重输出格式不正确|Invalid job priority output format
作业详情字段不明确|Ambiguous job detail fields
作业详情输出不正确|Invalid job detail output
只能删除当前登录用户自己的作业|Only jobs owned by the current SSH user can be cancelled
删除确认已失效，请重新确认|The cancellation confirmation has expired; confirm again
无法可靠确认作业所属用户|Cannot reliably verify the job owner
服务器没有返回作业权重|The server returned no job priority
服务器返回的作业 ID 与所选作业不一致|The job ID returned by the server does not match the selected job
请从当前服务器的最新作业列表中选择作业|Select a job from this server's current job list
连接已变化，请刷新作业列表后重试|The connection changed; refresh the job list and retry
squeue 作业行格式无法识别，请使用默认输出格式|Unrecognized squeue job row; use the default output format
squeue 未返回可识别的作业表头|squeue returned no recognizable job table header
squeue 表头无法识别，请使用默认输出格式|Unrecognized squeue header; use the default output format
squeue 输出格式不正确或超过限制|Invalid or oversized squeue output
监测服务尚未就绪，请稍后重试|The monitoring service is not ready; retry shortly
未安装或无法找到 nvidia-smi，无法判断是否存在 NVIDIA 显卡|nvidia-smi is unavailable; NVIDIA GPU presence cannot be determined
未检测到 NVIDIA 显卡|No NVIDIA GPU detected
nvidia-smi 显卡状态表不完整|The nvidia-smi GPU status table is incomplete
nvidia-smi 返回了不完整的 CSV 数据|nvidia-smi returned incomplete CSV data
nvidia-smi 进程表不完整，无法确认显卡是否空闲|The nvidia-smi process table is incomplete; GPU idle status cannot be confirmed
无法解析 nvidia-smi 显卡名称行|Cannot parse the nvidia-smi GPU name row
无法解析 nvidia-smi 显卡数据|Cannot parse nvidia-smi GPU data
无法解析 nvidia-smi 显卡编号|Cannot parse the nvidia-smi GPU index
无法解析 nvidia-smi 计算进程数据|Cannot parse nvidia-smi compute process data
无法解析 nvidia-smi 计算进程编号|Cannot parse the nvidia-smi compute process ID
无法识别 nvidia-smi 状态表|Cannot recognize the nvidia-smi status table
进程编号无效|Invalid process ID
服务器仅支持状态输出，已使用兼容模式|The server supports status output only; compatibility mode is active
/tmp 部分目录无法读取，只清理当前可访问的本人文件|Some /tmp directories cannot be read; only your currently accessible files will be cleaned
/tmp 部分目录无法读取，统计仅包括当前可访问内容|Some /tmp directories cannot be read; totals include accessible content only
个人目录部分内容无法读取，统计可能不完整|Some personal directories cannot be read; totals may be incomplete
无法读取临时目录，暂不能确认清理范围|Cannot read the temporary directory or confirm the cleanup scope
服务器不允许执行空间统计工具|The server does not allow storage measurement tools
服务器缺少 GNU find，无法统计个人空间|GNU find is missing on the server; personal storage cannot be measured
服务器返回的用户 UID 不一致，请手动检查清理结果|The returned user UID does not match; check the cleanup result manually
清理确认已失效，请重新确认|The cleanup confirmation has expired; confirm again
用户 UID 不正确|Invalid user UID
用户 UID 不正确或不允许清理 root 的临时文件|Invalid user UID; cleaning root's temporary files is not allowed
用户 UID 已变化或禁止清理 root 临时文件|The user UID changed or cleanup of root's temporary files is forbidden
临时目录不是可安全清理的实际目录|The temporary path is not a real directory that can be safely cleaned
用户名无法用于个人目录统计|The username cannot be used for personal storage measurement
空间占用合计超出范围|Total storage usage is out of range
空间统计数值超出范围|A storage measurement is out of range
空间统计文件数量不正确|Invalid storage file count
空间统计输出不完整|Incomplete storage measurement output
空间统计输出不正确|Invalid storage measurement output
空间统计输出格式不正确|Invalid storage measurement output format
空间统计返回了无效退出码|Storage measurement returned an invalid exit code
部分文件无法清理，可能存在权限限制或文件正在变化|Some files could not be cleaned due to permissions or concurrent changes
节点启动失败|Node boot failed
已取消|Cancelled
已完成|Completed
准备资源|Configuring resources
正在收尾|Completing
超过截止期限|Deadline exceeded
执行失败|Failed
节点故障|Node failure
内存不足|Out of memory
排队等待|Pending
排队中|Pending
等待中|Waiting
已被抢占|Preempted
运行中|Running
预约删除后挂起|Held after reservation deletion
联邦重新排队|Requeued by federation
重新排队并挂起|Requeued and held
重新排队|Requeued
调整资源规模|Resizing
联邦作业已撤回|Federated job revoked
正在发送信号|Signaling
特殊退出后挂起|Held after special exit
正在迁出数据|Staging out
已停止（保留资源）|Stopped (resources retained)
已暂停|Suspended
运行超时|Timed out
加急调度|Expediting
启动失败|Launch failed
等待节点启动|Waiting for nodes to boot
节点配置失败|Node configuration failed
正在更新作业数据库|Updating job database
未知状态|Unknown state
未知原因|Unknown reason
未知|Unknown
受计费策略限制|Restricted by accounting policy
账户无权使用该分区|The account is not allowed in this partition
关联配额的作业数已达上限|Association job limit reached
关联配额的资源已达上限|Association resource limit reached
关联配额的时长已达上限|Association time limit reached
关联配额的单用户运行作业数已达上限|Association running job limit per user reached
关联配额的单用户提交作业数已达上限|Association submitted job limit per user reached
作业资源条件无法满足|Job resource constraints cannot be met
尚未到最早启动时间|Earliest start time has not been reached
突发缓存操作失败|Burst buffer operation failed
突发缓存资源不足|Insufficient burst buffer resources
正在准备突发缓存数据|Staging burst buffer data
正在清理上次运行|Cleaning up the previous run
当前无法满足资源条件|Resource constraints cannot currently be met
无法按截止期限完成|The deadline cannot be met
等待依赖作业满足条件|Waiting for job dependencies
依赖条件无法满足|Job dependencies cannot be satisfied
等待联邦集群同步锁|Waiting for federation synchronization
超过作业无活动时长限制|Job inactivity limit exceeded
作业账户无效|Invalid job account
服务质量配置无效|Invalid quality of service
作业数组并发任务数已达上限|Job array concurrency limit reached
管理员挂起作业|Job held by administrator
用户或账户协调员挂起作业|Job held by user or account coordinator
重新排队次数已达上限|Maximum requeue count reached
作业启动失败|Job launch failed
等待许可证|Waiting for licenses
超过单处理器或单节点内存上限|Memory limit per CPU or node exceeded
所需节点已停机|Required nodes are down
作业以非零状态退出|Job exited with a nonzero status
等待本轮调度评估|Waiting for scheduling evaluation
作业内存不足|Job ran out of memory
违反分区配置限制|Partition configuration limit violated
分区已停机|Partition is down
分区未启用|Partition is inactive
不满足分区节点数限制或所需节点不可用|Partition node limit not met or required nodes unavailable
超过分区运行时长上限|Partition time limit exceeded
等待更高优先级作业|Waiting for higher-priority jobs
等待可用资源|Waiting for resources
等待预约资源|Waiting for reserved resources
指定节点当前不可用|Required nodes are currently unavailable
调度器延后即时资源分配|The scheduler deferred immediate resource allocation
调度系统或基础设施故障|Scheduling system or infrastructure failure
无权使用所选服务质量配置|The selected quality of service is not allowed
未满足服务质量使用量阈值|Quality of service usage threshold not met
服务质量配额的作业数已达上限|Quality of service job limit reached
服务质量配额的资源已达上限|Quality of service resource limit reached
服务质量配额的时长已达上限|Quality of service time limit reached
所选预约已被删除|The selected reservation was deleted
已用尽运行时长|The time limit was exhausted
等待调度器确定原因|Waiting for the scheduler to determine the reason
正在运行作业启动前脚本|Running the job prolog
关联配额|Association quota
服务质量配额|Quality of service quota
单用户|Per-user
单作业|Per-job
单节点|Per-node
总量|Total
处理器|CPUs
内存|Memory
节点数|Nodes
通用资源|Generic resources
许可证|Licenses
计费资源|Billing resources
能耗|Energy
突发缓存|Burst buffer
未识别资源|Unrecognized resources
提交作业数|Submitted jobs
运行作业数|Running jobs
运行中作业累计时长|Total running job time
累计使用时长|Accumulated usage time
运行时长|Time limit
请求|Requested
SSH 端口|SSH port
服务器地址|Server address
密钥路径|Private key path
检测间隔|Check interval
空闲利用率阈值|Idle utilization threshold
空闲显存阈值|Idle memory threshold
连续采样次数|Consecutive samples
设备名称|Server name
设备标识|Server ID
设备配置|Server settings
隧道名称|Tunnel name
隧道标识|Tunnel ID
隧道检测间隔|Tunnel check interval
监听地址|Bind address
监听端口|Bind port
目标地址|Target address
目标端口|Target port
用户名|Username
启用状态格式不正确|Invalid enabled state
确认结果格式不正确|Invalid confirmation result
Codex 工具尚未就绪，请稍后重试|The Codex tool is not ready; retry shortly
Codex 操作只接受设备标识|Codex operations accept only a server ID
Codex 返回了无法识别的版本号|Codex returned an unrecognized version
Codex 安装失败|Codex installation failed
Codex 更新失败|Codex update failed
Codex 检测结果格式不正确|Invalid Codex detection output format
Codex 检测结果超出限制|Codex detection output exceeded the limit
Codex 路径不是有效的 UTF-8|The Codex path is not valid UTF-8
Codex 检测结果包含无效字段|Codex detection output contains invalid fields
服务器未返回有效的 Codex 检测结果（需要 POSIX shell）|The server returned no valid Codex detection result (a POSIX shell is required)
发现多个未加入登录 PATH 的 Codex 安装，请先在服务器中选择要使用的 Node/Codex 版本|Multiple Codex installations are outside the login PATH; choose the Node/Codex version on the server first
Codex 安装检测结果不完整|Incomplete Codex installation detection result
Codex 安装方式检测结果不正确|Invalid Codex installation method detection result
Codex 安装目录不是有效的绝对路径|The Codex installation directory is not a valid absolute path
Codex 可执行文件路径无效|Invalid Codex executable path
npm 安装目录检测不完整|Incomplete npm installation directory detection
已发现 Codex，但无法读取其版本，请先在服务器中修复 Codex 运行环境|Codex was found but its version could not be read; repair the Codex environment on the server first
该服务器系统或处理器暂不支持官方 Codex 原生安装包|The server OS or CPU is not supported by official native Codex packages
该服务器系统或处理器暂不支持自动安装|Automatic installation is not supported for the server OS or CPU
当前 SSH 用户没有 Codex 安装目录的写入权限|The SSH user cannot write to the Codex installation directory
当前 SSH 用户没有 npm 全局安装目录的写入权限；请在服务器中调整权限后重试|The SSH user cannot write to the global npm directory; adjust its permissions on the server and retry
Codex 版本信息过大|Codex version information is too large
Codex 版本服务返回了无效数据|The Codex version service returned invalid data
Codex 版本服务返回了错误的软件包|The Codex version service returned an incorrect package
Codex 最新版本缺失|The latest Codex version is missing
Codex 最新稳定版信息不正确|Invalid latest stable Codex version information
Codex daemon 未返回版本信息|The Codex daemon returned no version information
Codex daemon 版本信息格式不正确|Invalid Codex daemon version information
没有确认到运行中的 Codex 自管 daemon|No running Codex-managed daemon was confirmed
Codex daemon 管理路径缺失|The Codex daemon management path is missing
Codex daemon 未归属于当前用户的 Codex 管理目录|The Codex daemon does not belong to the current user's Codex management directory
Codex daemon 运行版本缺失|The running Codex daemon version is missing
Codex daemon 未返回更新结果|The Codex daemon returned no update result
Codex daemon 更新结果格式不正确|Invalid Codex daemon update result
该服务器上的 Codex daemon 暂不支持官方自动更新，请在服务器中手动处理|This server's Codex daemon does not support official automatic updates; update it manually on the server
Codex daemon 返回了未知的更新状态|The Codex daemon returned an unknown update status
Codex 运行状态检测结果无效|Invalid Codex runtime detection result
Codex 服务 PID 无效|Invalid Codex service PID
Codex 服务启动时间无效|Invalid Codex service start time
Codex 服务标识无效|Invalid Codex service identity
Codex 运行状态检测结果格式不正确|Invalid Codex runtime detection output format
当前 Codex 安装方式不支持自动更新|The current Codex installation method does not support automatic updates
npm 安装目录已变化，请重新检查|The npm installation directory changed; check again
Codex 完整安装包传输不完整，请重试|The complete Codex package transfer was incomplete; retry
无法确认 Codex 安装方式，已避免覆盖|The Codex installation method could not be confirmed; existing files were preserved
服务进程已变化，请手动重启|The service process changed; restart it manually
服务启动命令已变化，请手动重启|The service start command changed; restart it manually
服务未返回有效的新进程|The service returned no valid new process
服务未使用本次更新的 Codex，请手动检查服务配置|The service is not using the updated Codex; check its configuration manually
Codex 完整安装包大小超出限制|The complete Codex package exceeds the size limit
Codex 完整安装包长度发生变化，请重试|The complete Codex package length changed; retry
SSH 连接已断开，Codex 操作已取消|SSH disconnected; the Codex operation was cancelled
正在通过本机下载并校验官方 Codex 完整安装包…|Downloading and verifying the complete official Codex package on this computer…
检测服务器 Codex 失败|Failed to detect Codex on the server
Codex 操作正在进行，请稍后重试|A Codex operation is in progress; retry shortly
当前 Codex 已是最新版本，无需更新|Codex is already up to date
正在按已识别的安装方式更新 Codex…|Updating Codex using the detected installation method…
正在使用官方安装脚本安装 Codex…|Installing Codex using the official installation script…
安装结束后无法读取 Codex 版本，请检查服务器环境|Cannot read the Codex version after installation; check the server environment
安装后的 Codex 版本未达到最新版本，请检查服务器网络或安装目录|The installed Codex version is outdated; check the server network or installation directory
安装后的 Codex 路径或管理方式发生变化，请在服务器中检查 PATH|The Codex path or management method changed after installation; check PATH on the server
Codex CLI 已更新；原 daemon 已停止或管理路径已变化，请在服务器中确认后重启|Codex CLI updated; the previous daemon stopped or its management path changed; verify and restart it on the server
Codex CLI 已更新，但 daemon 的运行版本未达到最新版本，请检查服务器中的 daemon 更新状态|Codex CLI updated but the running daemon is outdated; check the daemon update on the server
Codex CLI 已更新，但 daemon 更新后未确认恢复运行，请在服务器中检查|Codex CLI updated but the daemon was not confirmed running after updating; check it on the server
未发现当前用户运行中的 Codex；下次启动将使用新版本|No running Codex was found for the current user; the next launch will use the new version
Codex 已更新。交互式 Codex 请在原终端退出后重新打开；未确认可安全自动重启的用户服务|Codex updated. Exit and reopen interactive Codex in its terminal; no user service was confirmed safe to restart automatically
官方 daemon 更新命令失败|The official daemon update command failed
请检查服务|Check the service
：当前 SSH 用户的写入权限不足，请检查所选安装目录或 npm 缓存权限|: the SSH user lacks write permission; check the installation directory or npm cache permissions
：服务器访问官方下载地址失败，请检查服务器外网、DNS或代理配置后重试|: the server could not reach the official download URL; check its internet access, DNS, or proxy and retry
：下载安装脚本失败，请根据详细错误修复后重试|: downloading or running the installation script failed; resolve the detailed error and retry
Codex 上传文件大小超过限制|The Codex upload file exceeds the size limit
Codex 原生更新仅支持正式发布版本|Native Codex updates support stable releases only
Codex 发布包下载地址不属于所选官方版本|The Codex package URL does not belong to the selected official version
Codex 官方发布信息过大|Official Codex release information is too large
Codex 官方发布包大小无效或超过限制|The official Codex package size is invalid or exceeds the limit
Codex 官方发布版本与请求版本不一致|The official Codex release version does not match the requested version
Codex 官方校验文件格式无效|Invalid official Codex checksum file format
Codex 更新包 SHA256 校验失败，已停止安装|Codex package SHA256 verification failed; installation stopped
Codex 更新包下载不完整，请重试|The Codex package download is incomplete; retry
Codex 更新包下载数据超过官方声明大小|The Codex download exceeds its official declared size
Codex 更新包准备任务失败|Preparing the Codex package failed
Codex 更新包大小与官方发布信息不一致|The Codex package size does not match the official release information
Codex 更新包校验清单与官方 SHA256 不一致|The Codex package manifest does not match the official SHA256
下载 Codex 官方更新包超时，请检查本机网络后重试|The official Codex package download timed out; check this computer's network and retry
不支持该服务器的 Codex 原生安装平台|The server platform is not supported by native Codex packages
保存 Codex 官方发布信息失败|Failed to save official Codex release information
保存 Codex 更新包失败|Failed to save the Codex package
准备 Codex 更新包超时，请检查本机网络后重试|Preparing the Codex package timed out; check this computer's network and retry
完成 Codex 更新文件打包失败|Failed to finish packaging the Codex update
官方发布信息包含重复文件|Official release information contains duplicate files
官方发布信息缺少文件列表|Official release information is missing its file list
官方发布包的 SHA256 校验信息无效|Invalid official release package SHA256 information
官方发布包缺少有效的 SHA256 校验信息|The official release package has no valid SHA256 information
官方发布包重定向次数过多|Too many redirects for the official release package
官方发布文件名无效|Invalid official release filename
打包 Codex 更新文件失败|Failed to package the Codex update files
拒绝非官方 HTTPS 下载地址|Unofficial HTTPS download URL rejected
无法写入 Codex 更新包，请检查本机磁盘空间|Cannot write the Codex package; check this computer's disk space
无法创建 Codex 上传文件|Cannot create the Codex upload file
无法创建 Codex 更新临时文件，请检查本机临时目录空间和权限|Cannot create a temporary Codex update file; check the local temporary directory space and permissions
无法创建 Codex 更新临时目录|Cannot create the temporary Codex update directory
无法初始化 Codex 更新包下载|Cannot initialize the Codex package download
无法解析 Codex 官方发布信息|Cannot parse official Codex release information
读取 Codex 上传文件大小失败|Failed to read the Codex upload file size
读取 Codex 官方校验文件失败|Failed to read the official Codex checksum file
读取 Codex 更新文件大小失败|Failed to read the Codex update file size
读取 Codex 更新文件失败|Failed to read the Codex update file
当前没有可安装的新版本|There is no new version to install
正在准备软件更新|Preparing the application update
此构建尚未配置自动更新签名公钥|This build has no automatic update signature public key configured
此 Linux 运行方式请从 GitHub Release 手动下载新版本；官方 AppImage 支持自动更新|Download updates manually from GitHub Releases for this Linux installation; the official AppImage supports automatic updates
无法确定当前安装目录|Cannot locate the current installation directory
更新检查已取消|Update check cancelled
更新下载已取消|Update download cancelled
更新版本必须高于当前版本|The update version must be newer than the current version
更新包尚未准备好|The update package is not ready
更新包地址不属于 LionSSH 官方 GitHub Release|The update URL does not belong to an official LionSSH GitHub Release
更新签名版本与发布版本不一致|The signed update version does not match the release version
更新签名缺少版本信息|The update signature has no version information
更新清单路径不安全|Unsafe path in the update manifest
更新清单包含 Windows 设备路径|The update manifest contains a Windows device path
更新清单包含无效的路径段|The update manifest contains an invalid path segment
更新包不能修改用户 data 或更新工作目录|Updates cannot modify user data or the update working directory
便携包架构无效|Invalid portable package architecture
便携包文件清单数量无效|Invalid portable package manifest file count
便携包文件清单重复或自引用|The portable manifest contains duplicate or self-referencing entries
便携包文件校验值无效|Invalid portable package file checksum
更新包超过大小限制|The update package exceeds the size limit
更新包解压大小超过限制|The extracted update package exceeds the size limit
便携包缺少主程序或 portable.json|The portable package is missing the application or portable.json
更新 ZIP 文件数量过多|The update ZIP contains too many files
更新 ZIP 不能包含符号链接|The update ZIP must not contain symbolic links
更新 ZIP 包含重复路径|The update ZIP contains duplicate paths
更新 ZIP 大小无效|Invalid update ZIP size
更新 ZIP 解压大小超过限制|The extracted update ZIP exceeds the size limit
更新 ZIP 文件大小不一致|The update ZIP file size does not match
更新 ZIP 必须包含唯一的便携分发清单|The update ZIP must contain one portable distribution manifest
便携分发清单版本与更新版本不一致|The portable manifest version does not match the update version
便携更新包架构与当前程序不一致|The portable update architecture does not match the current application
便携更新 ZIP 含有清单外文件|The portable update ZIP contains files outside its manifest
便携标识缺失|The portable marker is missing
便携更新包的 portable.json 标识无效|The portable update's portable.json marker is invalid
无法确定便携版目录|Cannot locate the portable application directory
请将便携版主程序文件名恢复为 Lion SSH.exe 后更新|Restore the portable application filename to Lion SSH.exe before updating
更新路径包含重解析点|The update path contains a reparse point
更新路径包含符号链接|The update path contains a symbolic link
更新路径的父级不是目录|The update path's parent is not a directory
现有便携目录缺少有效 manifest.json，请先完整解压便携版|The portable directory has no valid manifest.json; fully extract the portable package first
受管程序文件被目录替代|A managed application file was replaced by a directory
便携文件名大小写发生变更，请手动更新该版本|Portable filename capitalization changed; update this version manually
等待旧程序退出超时，更新已取消|Waiting for the previous application to exit timed out; the update was cancelled
便携更新助手仅支持 Windows|The portable update helper supports Windows only
更新任务目录无效|Invalid update task directory
更新助手只接受自己的临时任务目录|The update helper only accepts its own temporary task directory
便携更新目标不是原程序目录|The portable update target is not the original application directory
便携更新版本必须高于当前程序|The portable update must be newer than the current application
更新清理任务与当前程序不匹配|The update cleanup task does not match the current application
更新清理目录包含额外文件，已保留|The update cleanup directory contains additional files and was preserved
更新助手尚未退出，临时目录已保留|The update helper is still running; its temporary directory was preserved
更新助手参数无效|Invalid update helper arguments
`.trim().split('\n').map(line => {
  const split = line.indexOf('|');
  return [line.slice(0, split), line.slice(split + 1)];
}));

// Third column lists error slots containing another app-authored error. Other
// slots, including names/paths and terminal signal details, are always verbatim.
const templates: [string, string, number[]?][] = [
  ['{0} 秒后重新连接；隧道规则保留', 'Reconnecting in {0} seconds; tunnel rules are retained'],
  ['{0}：隧道已建立', '{0}: tunnel established'],
  ['目标不可用：{0}', 'Target unavailable: {0}', [0]],
  ['读取私钥失败：{0}', 'Failed to read the private key: {0}', [0]],
  ['解析 SSH 私钥失败，请检查文件和口令：{0}', 'Failed to parse the SSH private key; check the file and passphrase: {0}', [0]],
  ['终端通道建立失败：{0}', 'Failed to open the terminal channel: {0}', [0]],
  ['读取上传文件失败：{0}', 'Failed to read the upload file: {0}'],
  ['上传文件失败：{0}', 'File upload failed: {0}'],
  ['文件传输完成前远程指令已退出（退出码 {0}）：{1}', 'The remote command exited before the transfer completed (exit code {0}): {1}'],
  ['旧远端监听未确认释放，将关闭 SSH 会话后恢复：{0}', 'The previous remote listener has not been released; the SSH session will close before recovery: {0}'],
  ['终端发送失败：{0}', 'Failed to send terminal input: {0}'],
  ['终端被信号 {0} 结束：{1}', 'Terminal terminated by signal {0}: {1}'],
  ['终端被信号 {0} 结束', 'Terminal terminated by signal {0}'],
  ['不可用节点：{0}', 'Unavailable nodes: {0}'],
  ['已请求取消自己的作业 {0}', 'Cancellation requested for your job {0}'],
  ['已清理自己的 /tmp 文件及空目录，共 {0} 项（部分内容未清理）', 'Cleaned {0} of your /tmp files and empty directories (some content remains)'],
  ['已清理自己的 /tmp 文件及空目录，共 {0} 项', 'Cleaned {0} of your /tmp files and empty directories'],
  ['查询失败（退出码 {0}）', 'Query failed (exit code {0})'],
  ['监听端口 {0} 在当前设备中重复', 'Duplicate bind port {0} on this server'],
  ['{0} 端口必须在 1 到 65535 之间', '{0} port must be between 1 and 65535'],
  ['无法创建数据目录 {0}：{1}', 'Cannot create the data directory {0}: {1}'],
  ['无法创建 data 目录：{0}', 'Cannot create the data directory: {0}'],
  ['无法读取配置 {0}：必须是 JSON 数组', 'Cannot read configuration {0}: a JSON array is required'],
  ['无法读取配置 {0}：必须是 JSON 对象', 'Cannot read configuration {0}: a JSON object is required'],
  ['无法读取配置 {0}：{1}', 'Cannot read configuration {0}: {1}', [1]],
  ['无法保存配置 {0}：{1}', 'Cannot save configuration {0}: {1}', [1]],
  ['{0}；恢复原配置失败：{1}', '{0}; failed to restore the original configuration: {1}', [0]],
  ['无法初始化版本检查：{0}', 'Cannot initialize version checks: {0}'],
  ['无法检查 Codex 最新版本：{0}', 'Cannot check the latest Codex version: {0}'],
  ['Codex 版本服务暂时不可用：{0}', 'The Codex version service is temporarily unavailable: {0}'],
  ['读取 Codex 版本信息失败：{0}', 'Failed to read Codex version information: {0}'],
  ['读取已校验的 Codex 完整安装包失败：{0}', 'Failed to read the verified complete Codex package: {0}'],
  ['完整安装包已校验（{0} MiB），正在通过 SSH 传送并安装…', 'Complete package verified ({0} MiB); transferring and installing over SSH…'],
  ['正在更新并重启运行中的 Codex daemon {0}…', 'Updating and restarting running Codex daemon {0}…'],
  ['Codex CLI 已更新，运行中的 daemon 已更新至 {0} 并确认可用', 'Codex CLI updated; the running daemon was updated to {0} and verified'],
  ['Codex CLI 已更新；daemon 已是 {0} 最新版并保持运行', 'Codex CLI updated; daemon {0} is already current and remains running'],
  ['Codex CLI 已更新，但 daemon 更新失败：{0}', 'Codex CLI updated, but the daemon update failed: {0}', [0]],
  ['Codex CLI 已更新；{0}', 'Codex CLI updated; {0}', [0]],
  ['正在重启已确认的用户服务 {0}…', 'Restarting verified user service {0}…'],
  ['已更新并重启用户服务 {0}', 'Updated and restarted user service {0}'],
  ['Codex 已更新，但用户服务重启未验证成功：{0}', 'Codex updated, but the user service restart could not be verified: {0}', [0]],
  ['Codex 已更新，服务重启失败：{0}', 'Codex updated, but the service restart failed: {0}', [0]],
  ['Codex 更新成功：{0}', 'Codex updated successfully: {0}'],
  ['Codex 安装成功：{0}', 'Codex installed successfully: {0}'],
  ['下载 Codex 官方更新包失败：{0}', 'Failed to download the official Codex package: {0}'],
  ['官方发布缺少 {0}', 'The official release is missing {0}'],
  ['便携更新包缺少 {0}', 'The portable update package is missing {0}'],
  ['便携更新文件校验失败：{0}', 'Portable update file verification failed: {0}'],
  ['程序文件 {0} 已被修改，请先备份后手动更新', 'Application file {0} was modified; back it up and update manually'],
  ['更新将覆盖用户文件 {0}，请先移走该文件', 'The update would overwrite user file {0}; move it first'],
  ['无法等待旧程序退出：{0}', 'Cannot wait for the previous application to exit: {0}'],
];

const escape = (value: string) => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
const patterns = templates.map(([source, translation, nested = []]) => {
  const indices: number[] = [];
  const parts = source.split(/(\{\d+\})/g).map(part => {
    const token = /^\{(\d+)\}$/.exec(part);
    if (!token) return escape(part);
    indices.push(Number(token[1]));
    return '([\\s\\S]+?)';
  });
  return { regex: new RegExp(`^${parts.join('')}$`), translation, indices, nested };
});

const quotaOwner = '(关联配额|服务质量配额)';
const quotaResource = '(突发缓存|计费资源|处理器|能耗|通用资源|运行作业数|许可证|内存|节点数|提交作业数|未识别资源|运行时长)';
const quotaMaximum = new RegExp(`^${quotaOwner}的(单作业|单节点|单用户)?${quotaResource}(总量|运行中作业累计时长|累计使用时长|请求)(已达上限|超过上限)$`);
const quotaMinimum = new RegExp(`^${quotaResource}请求未达到${quotaOwner}下限$`);
const quotaAccount = new RegExp(`^超过服务质量配额的单账户${quotaResource}上限$`);
const reasonLabels = new Set('受计费策略限制|账户无权使用该分区|关联配额的作业数已达上限|关联配额的资源已达上限|关联配额的时长已达上限|关联配额的单用户运行作业数已达上限|关联配额的单用户提交作业数已达上限|作业资源条件无法满足|尚未到最早启动时间|突发缓存操作失败|突发缓存资源不足|正在准备突发缓存数据|正在清理上次运行|当前无法满足资源条件|无法按截止期限完成|等待依赖作业满足条件|依赖条件无法满足|等待联邦集群同步锁|超过作业无活动时长限制|作业账户无效|服务质量配置无效|作业数组并发任务数已达上限|管理员挂起作业|用户或账户协调员挂起作业|重新排队次数已达上限|作业启动失败|等待许可证|超过单处理器或单节点内存上限|所需节点已停机|作业以非零状态退出|等待本轮调度评估|作业内存不足|违反分区配置限制|分区已停机|分区未启用|不满足分区节点数限制或所需节点不可用|超过分区运行时长上限|等待更高优先级作业|等待可用资源|等待预约资源|指定节点当前不可用|调度器延后即时资源分配|调度系统或基础设施故障|无权使用所选服务质量配置|未满足服务质量使用量阈值|服务质量配额的作业数已达上限|服务质量配额的资源已达上限|服务质量配额的时长已达上限|所选预约已被删除|已用尽运行时长|等待调度器确定原因|正在运行作业启动前脚本|未知原因'.split('|'));

function translateQuota(source: string): string | undefined {
  const maximum = quotaMaximum.exec(source);
  if (maximum) {
    const [, owner, scope, resource, duration, state] = maximum;
    return `${exact.get(owner)}: ${scope ? `${exact.get(scope)!.toLowerCase()} ` : ''}${exact.get(resource)!.toLowerCase()} ${exact.get(duration)!.toLowerCase()} limit ${state === '已达上限' ? 'reached' : 'exceeded'}`;
  }
  const minimum = quotaMinimum.exec(source);
  if (minimum) return `${exact.get(minimum[1])} request is below the ${exact.get(minimum[2])!.toLowerCase()} minimum`;
  const account = quotaAccount.exec(source);
  if (account) return `Quality of service ${exact.get(account[1])!.toLowerCase()} limit per account exceeded`;
  return undefined;
}

// The backend uses a semicolon between translated Slurm reasons. Only split
// when every component is recognized; unknown server details remain untouched.
function translateReason(source: string): string | undefined {
  const quota = translateQuota(source);
  if (quota) return quota;
  if (source.includes('；')) {
    const parts = source.split('；');
    const translated = parts.map(part => exact.get(part) || translateReason(part));
    if (translated.every(Boolean)) return translated.join('; ');
  }
  const detail = /^(.*?)：([\s\S]+)$/.exec(source);
  if (detail) {
    const label = (reasonLabels.has(detail[1]) ? exact.get(detail[1]) : undefined) || translateQuota(detail[1]);
    if (label) return `${label}: ${detail[2]}`;
  }
  return undefined;
}

/** Translate known native messages while preserving captured names, paths, and remote details. */
export function backendMessage(source: string | null | undefined, language: 'zh' | 'en'): string {
  if (!source) return '';
  if (language === 'zh') return source;
  const translated = exact.get(source);
  if (translated) return translated;
  const reason = translateReason(source);
  if (reason) return reason;
  // Validation labels come from a fixed set in model.rs; arbitrary text is never replaced.
  const validation = /^(.*?)格式不正确$/.exec(source);
  if (validation && exact.has(validation[1])) return `Invalid ${exact.get(validation[1])!.toLowerCase()}`;
  const missing = /^缺少(.+)$/.exec(source);
  if (missing && exact.has(missing[1])) return `${exact.get(missing[1])} is required`;
  const range = /^(.+?)应在 (\d+)–(\d+) 之间$/.exec(source);
  if (range && exact.has(range[1])) return `${exact.get(range[1])} must be between ${range[2]} and ${range[3]}`;
  const failure = /^(Codex 安装失败|Codex 更新失败|检测服务器 Codex 失败|官方 daemon 更新命令失败|请检查服务)(.*?)（退出码 ([^）]+)）：([\s\S]*)$/.exec(source);
  if (failure) return `${exact.get(failure[1])}${exact.get(failure[2]) || failure[2]} (exit code ${exact.get(failure[3]) || failure[3]}): ${failure[4]}`;
  for (const { regex, translation, indices, nested } of patterns) {
    const match = regex.exec(source);
    if (match) return translation.replace(/\{(\d+)\}/g, (_, index: string) => {
      const value = match[indices.indexOf(Number(index)) + 1];
      return nested.includes(Number(index)) ? backendMessage(value, language) : value;
    });
  }
  return source;
}
