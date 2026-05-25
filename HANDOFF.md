# vibe-bridge HANDOFF

## 2026-05-24 ConPTY 尺寸收敛：优先修 host 端 producer/consumer 不一致

用户按新诊断重新采集 `docs/logs.txt` 后，结论已经从“传输/FIFO 不明”推进到“VT100 producer 尺寸与板端 consumer 尺寸不一致”：

- 板端 hash 确认是诊断版：
  - `aikb_hid_input`: `209dd98477d0f075bc8442f0d8fdaf44d4682e2bb48a464c93fc3eff2e21a361`
  - `aikb_lcd_ui`: `9cc7b20fd9b6e050e0090f8059dad9798716bec6a510488ee9c18268c778e0db`
- HID 收到并成功转发：`rx vt100 sid=2 active=2 ... fwd_rc=0`，累计转发到 LCD FIFO。
- LCD 进入 terminal view 并读取 VT100：`diag picker focus sid=2 view=terminal`，`diag input view=terminal ... has_data=1`。
- LCD 曾出现非空 cells，但每轮 replay 末尾又回到 `cells=...->0`，只剩 row/col 上的闪烁光标。
- Windows 日志显示 `terminal-shim` 请求 `cols=78 rows=15`，但实际 `[launch] conpty size cols=138 rows=51 requested_cols=78 requested_rows=15`。这说明 Codex/TUI 按 138x51 生成全屏输出，而板端按 78x15 简化终端消费，当前黑屏更像尺寸错配后的擦除/滚动结果，不应继续优先怀疑 HID 丢包。

本轮 host 修正：

- `crates/vb-daemon/src/main.rs`
  - `launch/start/terminal-shim` 默认不再跟随当前 Windows Console 窗口尺寸，使用请求的板端尺寸 `78x15`。
  - 新增显式 `--follow-console-size`，需要桌面 1:1 时可手动恢复旧行为。
  - 日志增加 `follow_console_size=...`，后续产品验证应看到默认 `conpty size cols=78 rows=15 requested_cols=78 requested_rows=15 follow_console_size=false`。

已验证：

- `cargo fmt --package vb-daemon`
- `cargo test -p vb-daemon`：37 lib tests + 13 main tests passed
- `cargo check -p vb-daemon --target x86_64-pc-windows-gnu`
- `git diff --check -- crates/vb-daemon/src/main.rs`

未验证：

- 未在 native Windows 重新运行 `cargo run -p vb-daemon -- install-windows --terminal-profiles`。
- 未用新版 Windows daemon 上板复测黑屏是否消失。

下一步：

1. 在未被 Vibe 捕获的 PowerShell 里重新安装：
   `cargo run -p vb-daemon -- install-windows --terminal-profiles`
2. 关闭旧的被捕获 Windows Terminal 窗口，重新打开 wrapped profile，再进入 WSL/Codex。
3. 复测后看 `%LOCALAPPDATA%\vibe-bridge\vb-daemon.log`：`conpty size` 必须是 `78x15` 且 `follow_console_size=false`。如果仍黑屏，再继续定位 LCD parser 对具体 CSI/scroll 序列的处理。

## 2026-05-24 sid 生命周期修复：防止旧 agent 抢占复用 sid

### 2026-05-24 补充：黑屏未消失，已加分层诊断

用户复测后现象仍是进入 session 后刷一下就空屏，左下角 `_` 闪烁。最新日志结论：

- sid 污染已收敛：focus `sid=2` 正确映射到当前 `codex/launch-wsl-codex-60287`，不再跳旧 agent。
- host focus 有 `buffered=2507 replay=2514`，但现有板端日志不能证明 Vt100Stream 是否收到/转发/LCD 是否解析后清屏。
- Windows 日志仍有 `no Vibe HID 359f:2120 device found`，需要新日志确认 focus replay 是否成功送出。

已加诊断，不打印原始 terminal 内容：

- `crates/vb-daemon/src/lib.rs`
  - `flush_board_outbox()` 成功发送 `Vt100Stream` 后打印 `sent vt100 sid=... bytes=... frames=...`。
- `../../../../AIKB/LicheeRV-Nano-Build/middleware/v2/sample/aikb_hid_input/aikb_hid_input.c`
  - 对 `CMD_VT100_STREAM` 记录 rx/fwd/drop_dead/drop_inactive/write_retry/write_fail 计数。
- `../../../../AIKB/LicheeRV-Nano-Build/middleware/v2/sample/aikb_lcd_ui/aikb_lcd_ui.c`
  - 记录 picker focus、ctrl view、terminal input bytes、parser state、cursor、viewport/effective viewport、非空 cell 数、printable/clear 计数。

已验证并同步：

- `cargo fmt --package vb-daemon`
- `cargo test -p vb-daemon`：50 passed
- `cargo check -p vb-daemon --target x86_64-pc-windows-gnu`
- `make DEPFLAGS=` in `aikb_hid_input`
- `make -B DEPFLAGS=` in `aikb_lcd_ui`（第一次普通 make 因残留 RISC-V `kitty_graphics.o` 与 x86 链接失败，强制重编后通过）
- apptainer 交叉编译并同步两个板端二进制到 sample、overlay、buildroot target、install rootfs
- 新 SHA：
  - `aikb_hid_input`: `209dd98477d0f075bc8442f0d8fdaf44d4682e2bb48a464c93fc3eff2e21a361`
  - `aikb_lcd_ui`: `9cc7b20fd9b6e050e0090f8059dad9798716bec6a510488ee9c18268c778e0db`
- `git diff --check` 覆盖 host/board 修改文件。

下一步：用户替换/重启这两个新二进制并复现一次，再收集同一组日志。判断规则：

- host 有 `sent vt100`，HID 无 `rx vt100`：Windows HID/USB 发送或板端 gadget 收包问题。
- HID 有 `rx vt100` 但 `fwd_rc` 非 0：FIFO 写入/LCD reader 问题。
- HID 有 fwd，LCD 无 `diag input`：FIFO reader/open 时序问题。
- LCD 有 `diag input` 且 clears 增、cells 归零：LCD parser/clear/viewport 问题。

### 当前结论

用户提供的 `docs/logs.txt` 证明黑屏/闪一下问题不应继续优先归因于 LCD parser：

- `sid=2` 先分配给新 `codex/launch-wsl-codex-59262`，但后续 focus `sid=2` 映射回旧 `codex/launch-wsl-codex-58365`。
- `sid=3` 先分配给新 codex，下一行 focus `sid=3` 却映射到旧 `terminal/launch-48832`。
- 根因是 daemon 允许多个 registered agent 同时持有同一个 `board_sid`，`focus_board_sid()` 又按 `HashMap` 找第一个 owner，结果不稳定。
- 另一个噪声源是 board 把 host-level broadcast heartbeat `sid=0` 当 invalid 回报，daemon 原先又忽略 `SessionInvalid`。

### 已改

- `crates/vb-daemon/src/lib.rs`
  - `SESSION_RESPONSE sid=N` 绑定新 agent 前先解绑其它持有同一 sid 的旧 agent。
  - `SESSION_INVALID sid=N` 会清理对应 owner 和 focused sid；`sid=0` 作为 broadcast invalid 忽略。
  - 新增单测覆盖 sid 复用、`SessionInvalid` 清理、broadcast invalid 不误清理。
- `../../../../AIKB/LicheeRV-Nano-Build/middleware/v2/sample/aikb_hid_input/aikb_hid_input.c`
  - `CMD_SESSION_HEARTBEAT sid=0` 作为 host-level heartbeat 直接忽略，不再回 `SESSION_INVALID sid=0`。
  - 已交叉编译并同步 `aikb_hid_input` 到 sample、overlay、buildroot target、install rootfs；四处 SHA 一致：`607e1e54310900a94fddd1bb6968a4aedf57cda8230bea7880663e42883141bc`。

### 已验证

- `cargo fmt --package vb-daemon`
- `cargo test -p vb-daemon`：50 passed
- `cargo check -p vb-daemon --target x86_64-pc-windows-gnu`
- `make DEPFLAGS=` in `middleware/v2/sample/aikb_hid_input`
- apptainer 交叉编译 `aikb_hid_input` 并同步四处 RISC-V stripped binary
- `git diff --check -- crates/vb-daemon/src/lib.rs`
- `git diff --check -- middleware/v2/sample/aikb_hid_input/aikb_hid_input.c`

### 未验证

- 未在 native Windows 重新安装/启动最新 `vb-daemon`。
- 未上板复现验证黑屏是否消失。
- 未运行 `pack_rootfs` / `pack_burn_image` / 烧录。

### 下一步

1. Windows 侧重新安装/启动最新 daemon。
2. 板端替换/重启已同步的 `aikb_hid_input`，或重新 pack/烧录。
3. 复现进入 codex sid，观察 `vb-daemon.log` 不应再出现同 sid focus 到旧 agent；`/tmp/aikb_hid_input.log` 不应继续刷 `SESSION_INVALID sid=0`。

## 2026-05-24 30 秒接手：WSL/Codex 注册已通，板端进入 session 后闪现再变空屏

### 当前 100% 定义

本阶段 100% 不是“有 session”或“能闪一下”，而是：

1. Windows 上一次安装/升级后，不改变用户平时使用习惯。
2. 新开的 Windows Terminal PowerShell、WSL/Ubuntu profile、Start Menu WSL 入口能进入捕获链路。
3. WSL 内用户照常输入 `codex` / `claude`，能生成内层 agent-aware sid。
4. 板端可选择正确 sid，并稳定显示当前 live TUI，不是 transcript 摘要，也不是闪一下后空屏。
5. Claude permission confirm/reject 仍可沿 hook 链路闭环。
6. 不引入终端重复输入、跳行、卡死、杀用户 Windows Terminal 等 UX 回归。

当前进度判断：结构链路约 80%-85%，但可见 TUI 稳定显示未完成，不能再按 98% 对外表达。

### 目标

主线目标仍是 native Windows 产品链路跑通：用户正常打开 Windows Terminal / Ubuntu / WSL，照常运行 agent，AIKB 板端能稳定显示真实终端/Codex TUI，并能用于后续权限确认。

### 状态

- Host 侧已做过并通过部分实测：
  - Windows installer 改为版本化 daemon exe：`%LOCALAPPDATA%\vibe-bridge\bin\vb-daemon-<hash>.exe`，规避正在运行 exe 被锁导致升级失败或卡住。
  - Windows Terminal profile wrapping 已能覆盖 PowerShell 和 WSL profile。
  - 捕获 PowerShell 会临时前置 shim dir，`where.exe wsl` 第一条命中 captured `wsl.cmd`。
  - 通过 PowerShell 输入 `wsl` 后，WSL 内可见 `VIBE_BRIDGE_TERMINAL_AGENT_ID=launch-...`。
  - 捕获 WSL 内 `PATH` 第一段为 `/tmp/vibe-bridge-shims-<launch>-<pid>`，`which codex` 命中临时 shim，`codex --version` 输出 `codex-cli 0.133.0`。
  - 直开未被 Windows Terminal settings 捕获的 Ubuntu/WSL 时，`VIBE_BRIDGE_TERMINAL_AGENT_ID` 为空、`which codex` 仍是用户原始路径；这是当前架构边界，不应伪装成已捕获。
  - 用户打开一个 Codex 后，板端出现过一个 session，说明 agent 注册/board session 分配链路至少部分走通。
- Board 侧当前最新用户现象：
  - 按编码器确认进入对话框后，终端内容显示约 200ms。
  - 约 1s 后变成黑色空屏，不是白屏；只有背光，没有文字。
  - 左下角上方两三行附近有 `_` 光标，以约 1s 有、1s 无的节奏闪烁。
  - 用户认为固件大概率已更新，因为字体大小和行为有变化。
- 最新已同步 board `aikb_lcd_ui` 二进制 hash：
  - `f79b8a426326a34407c530456f884b9f9c493980c7a6cb6f59554f94b1bd329f`
- 已运行过的验证：
  - host: `cargo test -p vb-daemon`，48 passed。
  - host: `cargo check -p vb-daemon --target x86_64-pc-windows-gnu`，passed。
  - board: `make DEPFLAGS=` in `middleware/v2/sample/aikb_lcd_ui`，passed，仅有既有 unused warnings。
  - board: RISC-V cross build + strip + 同步到 source binary、overlay、buildroot target、install rootfs，四处 hash 曾确认一致。

### 误差

当前问题不再是“完全没有 session”或“WSL shim 完全不生效”。误差已经收敛到：

- host/board 至少拿到过首帧，所以链路不是全断。
- 进入 session 后有后续刷新把可见文字清掉，或 parser/viewport/focus 在后续帧后进入“有光标但无文字 cell”的状态。
- 继续凭肉眼猜 ANSI parser 已经不合格；上一次修 parser 后现象只轻微变化，未解决主问题。

可能原因只能作为假设，不能当结论：

- host focus/replay 后又发送了 clear/空 repaint，attached sid 的 live buffer 为空或被错误覆盖。
- board `aikb_hid_input` 收到了 VT100 包，但没有按当前 active sid 送到 LCD FIFO。
- board `aikb_lcd_ui` parser 被 OSC/DCS/CSI 状态吞掉 printable bytes。
- `ESC[2J` / alt-screen / viewport 逻辑清屏后没有等到可见 printable 字符。
- viewport top/effective top 选到了空行区域。
- active sid/focus 在进入后发生错配。

### 下一步控制动作

不要继续盲目补 ANSI parser。下一轮最小闭环应先加可区分责任的计数器/日志，然后再修代码：

- host daemon：
  - focus sid、kind/id、attached parent。
  - focus replay bytes、live stream bytes after focus。
  - replay buffer 是否为空。
  - stream 中 `ESC[2J` / alt-screen / printable byte 的计数和附近 hex/sample。
- board `aikb_hid_input`：
  - 当前 active sid。
  - VT100 packet count / byte count。
  - 每个 sid 最近一次 packet 时间和长度。
  - 写入 LCD FIFO 的字节数。
- board `aikb_lcd_ui`：
  - received bytes。
  - full clear count / pending clear count。
  - printable chars count。
  - parser state。
  - non-empty cell count。
  - viewport top / effective viewport top。
  - 当前 focused sid。

只有这些计数器能区分“没收到、收到但没转发、parser 吞了、清屏了、viewport 错了、sid 错了”。

### 下次验证流程

如果继续调试，先不要让用户重新烧录一个无诊断差异的固件。应先提交带诊断的 host/board 最小改动，然后让用户验证一次。

Windows 复现后复制 host log：

```powershell
cd C:\Serein_Y\Sipeed\rv_nano\tools\vibe-bridge
Get-Content "$env:LOCALAPPDATA\vibe-bridge\vb-daemon.log" -Tail 260 | Set-Content .\docs\logs.txt -Encoding utf8
```

板端 shell 需要用户协助时给这组命令：

```sh
sha256sum /mnt/system/usr/bin/aikb_lcd_ui
pidof aikb_lcd_ui aikb_hid_input
tail -n 80 /tmp/aikb_lcd_ui.log
tail -n 160 /tmp/aikb_hid_input.log
ls -l /tmp/aikb_lcd_ui.in /tmp/aikb_lcd_ui.ctrl /tmp/aikb_ui_ctrl.in /tmp/aikb_pet_events.in
```

### 禁止重复的坑

- 不要再建议 `Stop-Process -Name WindowsTerminal -Force`。这会杀掉用户当前 Codex 所在终端，是严重 UX 事故。
- 安装/升级应从未被捕获的 PowerShell 执行：`Win+R -> powershell.exe -NoLogo -NoProfile`。
- 不要把 Windows PowerShell、捕获 PowerShell、WSL bash、直开 Ubuntu 混成一个上下文；每条命令必须说明在哪个 shell 跑。
- 不要把用户描述的“黑色空屏、只有背光和 `_` 光标”说成白屏。
- 用户问“你哪里改代码了”时，必须给文件、函数/关键词、diff 或 line anchor，不能只口头说改了。
- 重复两轮视觉修复失败后必须加诊断计数器，不能继续凭感觉 patch。
- 不要让用户反复烧录没有新观测能力的固件。

## 2026-05-24 当前闭环：终端镜像已通，下一步验证 agent-aware 内层 session

### 目标

主线目标仍是 native Windows 产品链路跑通：

1. `install-windows --terminal-profiles` 安装/升级稳定，不杀用户正在使用的 Windows Terminal。
2. 新打开的 Windows Terminal profile 被 ConPTY 捕获，板端能实时显示终端 VT100。
3. 在被捕获 PowerShell 内执行 `codex` / `claude` 时，经 Windows shim 进入 `agent-shim`，生成独立 agent-aware sid，而不只是外层 `Terminal` sid。
4. Claude hook 的 permission 请求应绑定到同一个 launch sid，板端 CONFIRM/REJECT 能回到 hook poll。

### 当前状态

- 用户已实测外层 Windows Terminal 捕获链路可用：`Write-Host VIBE_BOARD_E2E`、输入/删除 `1212` 能几乎实时显示到板端。
- 最新 `docs/logs.txt` 反馈：`where.exe codex` / `where.exe claude` 已命中 `%LOCALAPPDATA%\vibe-bridge\bin\*.cmd` shim，但 `codex --version` 输出 `The system cannot find the path specified.`，日志没有出现内层 `[launch] ... kind=codex`。
- 后续定位：`where.exe codex` 返回 `\\?\C:\Users\...\codex.cmd`，原因是 installer 写 User PATH 时用了 `canonicalize()`，Windows 会产生 Win32 verbatim 路径；`.cmd` 通过这种 PATH 命中后可能在执行 batch 前失败，导致没有 `[agent-shim] start`。
- 已修：写入 User PATH 和生成 `codex.cmd` / `claude.cmd` 时去掉 `\\?\` / `\??\` 前缀，保持 shell 兼容的普通 `C:\...` 路径。
- 最新日志确认 `agent-shim` 已启动并进入 attached mode：
  - `[agent-shim] start ... kind=codex captured=true parent=launch-50008 args=1`
  - `[agent-shim] attached ... command0=wsl.exe`
  - 但退出 `code=127`，用户侧输出 `real  not found in WSL`；根因是 WSL fallback 脚本用 `$1` 取 agent 名称，在现场 `$1` 为空。
- 已修：WSL fallback 改为 `wsl.exe --cd ~ --exec bash -lc <script> codex ...`，脚本用 `$0` 作为 agent 名称、`$@` 作为剩余参数，避免 `codex` 被吃掉。
- 最新日志确认 `codex --version` 已成功输出 `codex-cli 0.133.0`，`agent-shim attached ... code=0`，但仍没有 `[register] codex/launch-...`。
- 根因：product daemon 的 TCP registration accept loop 是单连接阻塞处理；外层 terminal launch 长连接占住 accept 线程后，内层 agent-shim 连接只进入 TCP backlog，daemon 没处理，所以没有注册 codex sid。
- 已修：`run_tcp_hid_daemon` 对每个 registration TCP client 单独 `thread::spawn`，共享 `Arc<Mutex<BridgeDaemon>>` 和 HID transport，避免 terminal 长连接阻塞后续 agent/hook 连接。
- 最新验证通过：新开 Windows Terminal PowerShell 后运行 `codex --version`：
  - `codex-cli 0.133.0`
  - `[agent-shim] start ... kind=codex captured=true parent=launch-41112`
  - `[agent-shim] attached ... agent_id=launch-19084 parent=launch-41112 command0=wsl.exe`
  - `[register] codex/launch-19084 name=codex from_launch=true existing=false`
  - `[board] request session for codex/launch-19084 hint=codex`
  - `[board] session response sid=39 for codex/launch-19084`
  - `[agent-shim] attached exit ... code=0`
  结论：外层 Terminal sid + 内层 Codex agent-aware sid 主链路已首次跑通。
- 产品方向修正：PowerShell 里转进 WSL 只能作为探针/兼容 fallback，不应作为 WSL 用户主入口。用户正常应打开 Ubuntu/WSL profile，在 WSL shell 内照常输入 `codex` / `claude`。
- 已新增 WSL profile 临时注入：当被包装的 Windows Terminal profile 是简单 `wsl.exe [-d DISTRO]` 时，`terminal-shim` 启动 WSL 前会改写为 `wsl.exe [-d DISTRO] --cd ~ --exec bash -lc <entry>`；entry 在 WSL `/tmp/vibe-bridge-shims-<terminal-id>-<pid>` 创建 transient `codex` / `claude` shim，临时前置 PATH，然后进入用户 `$SHELL -i`。
- 该 WSL transient shim 只对当前被捕获的新 WSL 终端生效，不写 `~/.local/bin`，不改 `.bashrc` / `.zshrc`，窗口关闭后自然失效。
- WSL transient shim 直接用 bash `/dev/tcp` 向 Windows daemon 发送 `agent.register` / `session.abort`，parent 绑定到外层 terminal sid，因此用户在 WSL profile 里照常输入 `codex` 应生成内层 `codex` sid。
- 用户反馈新 bug：在捕获 PowerShell 内输入 `where.exe codex` 等命令时偶发整个终端卡死，Ctrl+C/Enter 都无效，新建窗口恢复。
- 早期明显 UX 回归已修正：重复输入、`>>` 续行、光标跳行、安装 exe 被锁导致 overwrite/hang。
- Windows installer 当前使用版本化 daemon exe：`%LOCALAPPDATA%\vibe-bridge\bin\vb-daemon-<hash>.exe`，避免正在运行的 daemon 锁住固定 `vb-daemon.exe`。
- `terminal-shim` / `run_launch(kind=terminal)` 会把安装 shim 目录临时前置到子 shell `PATH`，并注入 `VIBE_BRIDGE_DAEMON=<addr>`，因此新 PowerShell 不依赖用户手动刷新 PATH。
- `run_launch(kind!=terminal)` 会注入 `VIBE_BRIDGE_LAUNCH_AGENT_ID=launch-<pid>`；Claude hook 优先用这个 id，使 live ConPTY 和 permission hook 绑定同一 sid。
- 修正方向：捕获终端内执行 `codex.cmd` / `claude.cmd` 时不再启动第二层 ConPTY，改为 attached agent：
  - agent shim 注册内层 `codex` / `claude` sid；
  - 真实命令直接继承当前 ConPTY stdio，避免两个 reader 抢同一个控制台输入；
  - daemon 把外层 terminal 的 VT100 stream 镜像给 attached agent sid，所以 picker 选择内层 sid 也能看到同一终端画面。
- 修正另一个卡死源：如果后台 daemon/TCP 被安装流程重启或断开，terminal stdout pump 不能退出；现在只禁用板端 stream 发送，继续 drain ConPTY 并写回本地终端，保证用户 PowerShell 不因板端链路断开而卡死。
- WSL fallback 现在通过 `wsl.exe --cd ~ -- bash -lc ...` 启动，避免 Windows 当前目录在 WSL 里不可映射时触发 `The system cannot find the path specified.`。
- WSL fallback 不是硬编码用户机器名：脚本按 `$HOME/.local/share/vibe-bridge/real-bin/$cmd`、`$HOME/.local/bin/$cmd`、系统路径和 `command -v` 查找，并跳过旧 vibe wrapper。
- 板端代码已支持 `session N hint TEXT`，SESSION picker 优先显示 host hint；终端底栏已从硬编码 `Claude` 改成动态 `Terminal/Codex/Claude #sid`。
- host repo 根目录没有 `CLAUDE.md`；已读项目 `AGENTS.md`、`HANDOFF.md`、共享 `KNOWN_FAILURES.md`，并运行 preflight。

### 本轮新增校验

- 新增单测 `wsl_agent_exec_script_is_generic_and_skips_old_wrapper`：
  - 锁住 WSL fallback 不含 `Administrator`、`Serein_Y`、`Ubuntu` 等本机/发行版硬编码。
  - 锁住跳过旧 `vibe-bridge WSL agent wrapper` 的行为。
- 新增单测 `terminal_stream_mirrors_to_attached_agent`：
  - 锁住 parent terminal 的 VT100 stream 会同步到 attached codex sid。
  - 防止后续回退到 nested ConPTY 抢输入方案。
- 新增单测 `strip_windows_verbatim_prefix_keeps_shell_compatible_path`：
  - 锁住 `\\?\C:\...` 和 `\??\C:\...` 会写回普通 shell 路径。
- 更新 `wsl_agent_exec_script_is_generic_and_skips_old_wrapper`：
  - 锁住 WSL fallback 使用 `$0` 承载 agent 名称，不再依赖现场为空的 `$1`。
- 新增单测 `wsl_terminal_profile_gets_transient_agent_shims`：
  - 锁住 WSL profile 会注入 transient codex/claude shim。
  - 明确不写 `.bashrc` 和 `.local/bin`。

### 已验证

- `/usr/bin/python3 /home/rv_nano/Sipeed/T_tools/agent_preflight.py --project vibe-bridge`
- `cargo fmt --package vb-daemon`
- `cargo test -p vb-daemon`：43 tests passed
- `cargo check -p vb-daemon --target x86_64-pc-windows-gnu`
- `git diff --check -- crates/vb-daemon/src/main.rs crates/vb-daemon/src/lib.rs adapters/claude-code-hook/index.js HANDOFF.md`

### 未验证

- 未在 native Windows 重新运行最新安装。
- 未验证在捕获 PowerShell 内执行 `codex` / `claude` 后是否出现内层 agent-aware sid。
- 未验证 Claude permission 请求与 `launch-<pid>` sid 的 CONFIRM/REJECT 闭环。
- 未重新烧录板端固件；当前建议仍是先完成 host agent-aware 主链路，再决定是否烧录。

### 下一步最小 Windows 验证

必须从未被 vibe-bridge 捕获的 PowerShell 执行安装，避免 installer 杀到承载当前交互的终端：

```powershell
Win+R -> powershell.exe -NoLogo -NoProfile
cd C:\Serein_Y\Sipeed\rv_nano\tools\vibe-bridge
cargo run -p vb-daemon -- install-windows --terminal-profiles
```

然后打开新的 Windows Terminal PowerShell，执行：

```powershell
where.exe codex
where.exe claude
codex --version
```

预期：

- `where.exe codex` / `where.exe claude` 第一条应是 `%LOCALAPPDATA%\vibe-bridge\bin\codex.cmd` / `claude.cmd`。
- `%LOCALAPPDATA%\vibe-bridge\vb-daemon.log` 应出现外层：
  - `[terminal-shim] start ...`
  - `[launch] ... kind=terminal ...`
  - `[register] terminal/launch-...`
- 执行 `codex --version` 或进入 `codex` 后，日志应出现内层：
  - `[agent-shim] attached ... kind=codex ...`
  - `[register] codex/launch-...`
- 板端 SESSION picker 应同时能看到外层 `Terminal: Windows PowerShell #pid` 和内层 `codex` / `claude` session。

如果 `where.exe codex` 第一条不是 shim，先不要继续测 agent；回到 PATH/shim 注入问题。
如果捕获 PowerShell 再次卡死，先新开窗口导出 `%LOCALAPPDATA%\vibe-bridge\vb-daemon.log`，不要强杀 Windows Terminal。

## 2026-05-23 最新 30 秒接手：板端空白，当前卡在 Windows Terminal profile 未命中捕获入口

### 目标

用户的最终目标没有变：Windows 产品环境只启用一次，不要求每个目录、每个 agent、每个用户工作区单独安装；普通 `codex` / `claude` 功能不能被破坏；板端要显示真实终端/Codex TUI，效果接近 `images/logs/v18.png`，不能再显示 `images/logs/v17.png` 那种 transcript 摘要/历史对话。

### 状态

- 用户已在 Windows 运行多次：
  ```powershell
  cargo run -p vb-daemon -- install-windows --terminal-profiles
  ```
  最新安装输出显示：
  - `daemon now : running`
  - `wsl install: skipped`
  - `startup    : installed`
  - `terminal   : wrapped 6 profile(s) in 1 settings file(s)` 或后续 `already installed or no supported profiles`
  - `user PATH  : shim dir ensured; open a new terminal to use it`
- 用户反馈：新建 Codex 后板端仍为空。
- 用户提供的 `docs/logs.txt` 最新关键内容：
  - 第 1 行用户在 WSL bash 里执行了 PowerShell 语法 `$env:WT_PROFILE_ID; $env:WT_SESSION; $PWD`，说明至少一次验证是在 WSL bash 语境，不是 PowerShell 语境。
  - 第 160-165 行显示最新 daemon 已启动并找到 HID：
    - `vb-daemon HID: \\?\hid#vid_359f&pid_2120&mi_04...`
    - `vb-daemon registration ipc: tcp://127.0.0.1:8765`
    - `passive discovery: disabled for board UI...`
  - 日志里没有任何 `[register] ...`，也没有 `[board] request session ...`。
- 当前代码里的事实：`terminal-shim` 只要被 Windows Terminal profile 真正启动，会经 `run_launch()` 发 `agent.register`；daemon 收到后必然打印 `[register] terminal/...`，随后 enqueue `REQUEST_SESSION` 并打印 `[board] request session ...`。

### 误差

板端空不是 Codex 内容解析问题，也不是 HID 首要问题。HID 已被 daemon 识别，passive discovery 已关闭，board 没 session 的直接原因是 active ConPTY capture path 没有注册到 daemon。

当前最可能卡点：

1. Windows Terminal 实际打开的 profile 没有使用被改写的 `commandline`。
2. Windows Terminal 使用了另一个 settings 文件或动态 profile，没有命中当前 installer 改写的文件。
3. `terminal-shim` 启动后立即失败，但失败没有写入 daemon log，所以当前证据看起来像“没有启动”。
4. 用户验证命令在 WSL bash 里跑了 PowerShell 语法，说明验证上下文可能和预期 Windows Terminal profile 不一致。

### 控制动作

不要重启 passive discovery，不要回到 transcript fallback，不要再替换 `~/.local/bin/codex` / `~/.local/bin/claude`。

下一步最小控制变量只能是“观测 Windows Terminal profile 是否命中 `terminal-shim`”：

1. 给 `terminal-shim` 增加最小启动自诊断日志：进程一进入 `run_terminal_shim()` 就向 `%LOCALAPPDATA%\vibe-bridge\vb-daemon.log` 追加一行，例如 `[terminal-shim] start pid=... name=... cmdline=...`。这样即使连不上 daemon 或注册失败，也能确认 profile 是否命中 shim。
2. 或先不改代码，直接让用户在 Windows PowerShell 读取实际 settings/profile commandline：
   ```powershell
   $paths = @(
     "$env:LOCALAPPDATA\Packages\Microsoft.WindowsTerminal_8wekyb3d8bbwe\LocalState\settings.json",
     "$env:LOCALAPPDATA\Packages\Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe\LocalState\settings.json",
     "$env:LOCALAPPDATA\Microsoft\Windows Terminal\settings.json"
   )
   foreach ($p in $paths) {
     if (Test-Path $p) {
       "=== $p ==="
       Select-String -Path $p -Pattern "terminal-shim|vb-daemon|commandline|name" -Context 0,1
     }
   }
   ```
3. 同时查进程：
   ```powershell
   Get-CimInstance Win32_Process -Filter "Name = 'vb-daemon.exe'" |
     Select-Object ProcessId, ExecutablePath, CommandLine
   ```

### 反馈

判断分支：

- 如果打开新 Windows Terminal 后 log 出现 `[terminal-shim] start`，但没有 `[register]`：查 `terminal_shim_command()` / ConPTY spawn / daemon TCP connect。
- 如果没有 `[terminal-shim] start`：查 Windows Terminal settings 路径、profile 实际 commandline、用户打开的是不是被包装 profile。
- 如果有 `[register]` 但没有 `[board] request session`：查 daemon `agent.register` 分支。
- 如果有 `[board] request session` 但没有 `[board] session response`：查 HID/板端 `REQUEST_SESSION -> SESSION_RESPONSE`。
- 如果有 session response 但板端空：查板端 UI/FIFO/focus/replay。

### 验证

完成下一步代码或诊断后，最小验证顺序：

1. Windows 重新安装：
   ```powershell
   cargo run -p vb-daemon -- install-windows --terminal-profiles
   ```
2. 关闭全部旧 Windows Terminal 窗口，打开一个新 profile。
3. 查看日志：
   ```powershell
   Get-Content "$env:LOCALAPPDATA\vibe-bridge\vb-daemon.log" -Tail 240
   ```
4. 预期至少看到：
   - `[terminal-shim] start ...`
   - `[register] terminal/...`
   - `[board] request session ...`
   - 有板端响应时再看到 `[board] session response sid=...`

### 沉淀

- passive transcript scan 已被证伪为 1:1 终端还原方案，只能作为发现/摘要/fallback。
- Windows 一次启用的产品路径仍可继续，但必须以 Windows Terminal profile / ConPTY capture 为主，而不是每个 agent 的 wrapper。
- 网络网关方向可以作为后续语义/用量/权限数据通道思考，但它不能替代 terminal VT100 还原：即使拦截 API 消息，也还原不了 Codex TUI 的屏幕状态、光标、增量 repaint、选择框和终端控制序列。

## 2026-05-23 最新 30 秒接手：Windows Terminal capture profile

当前结论：最终目标可以继续做，但主方案从“发现/替换每个 agent”改成“Windows 一次性接入终端 profile，daemon 捕获整个 shell 的 ConPTY VT100 字节流”。普通 `codex` / `claude` binary 不能被替换；WSL wrapper 仍保持默认不启用。

最新用户实测校正：

- 用户在 `rv_nano` 新开 codex 时板端没立刻出现；关闭后在 `slam` 新开 codex，板端同时新增两个 codex；点进 Typora 对话框看到 `images/logs/v17.png`，这是 passive transcript fallback 的历史对话渲染，不是目标 `images/logs/v18.png` 那种真实 Windows Terminal / Codex TUI。
- 结论：passive discovery 仍在注册 stale/delayed codex rows，且 passive replay 还把 transcript 文本伪装成终端画面。这与“终端原样显示”目标冲突。
- 已修：`run_tcp_hid_daemon` 默认不再启动 passive discovery 到板端 UI；如需旧 fallback，必须显式设置 `VIBE_BRIDGE_PASSIVE_DISCOVERY=1`。即使显式启用 passive，focus passive sid 也只显示“no live terminal capture”占位，不再渲染 transcript 历史文本。
- 已修：新包装 profile 的 capture session 名称改为 `Terminal: <profile>`，降低和 passive codex/Typora 行混淆的概率。已安装的旧 profile 名称可能保持原样，但功能走新 exe。
- 已修：重复执行 `install-windows --terminal-profiles` 会刷新已经包装过的 terminal profile，从旧 `--cmdline-b64` 里解出原始 profile 命令，再用当前安装 exe 和 `Terminal: <profile>` 名称重写；不再因为命令行里已有 `terminal-shim` 就直接跳过。

本轮新增代码：

- `crates/vb-daemon/src/main.rs`
  - 新增 `terminal-shim` / `capture-shell` / `vibe-terminal` 命令。
  - `terminal-shim` 会确保 Windows daemon 在 `127.0.0.1:8765` 监听，然后用 ConPTY 启动原 profile shell，把完整 stdout VT100 经 `terminal.stream` 送到 daemon/板端。
  - 新增 `install-windows --terminal-profiles`，扫描 Windows Terminal stable/preview/unpackaged settings，给可识别 profile 写入 `terminal-shim --cmdline-b64 <原 commandline>`。写入前会备份 `settings.json.vibe-bridge-backup.<epoch>`。
  - 修复 Windows `install-windows` 遇到 `%LOCALAPPDATA%\vibe-bridge\bin\vb-daemon.exe` 被旧 daemon 锁住时直接失败的问题：installer 现在只停止安装目录里同路径的旧 `vb-daemon.exe`，避开当前 `cargo run` 进程，然后重试复制。
  - 第一次 stop 逻辑仍未命中用户现场旧进程；已改为用 `Get-CimInstance Win32_Process -Filter "Name = 'vb-daemon.exe'"` 同时匹配 `ExecutablePath` 和 `CommandLine`，并等待旧进程消失后再重试复制。
  - 新包装 Windows Terminal profile 的 `--name` 改成 `Terminal: <profile>`。
  - 对已经包装过的 Windows Terminal profile 支持刷新，而不是跳过。
  - 支持 JSONC 注释剥离、动态 PowerShell/WSL/Command Prompt profile 的最小 commandline 推断。
- `crates/vb-core/src/lib.rs`
  - 新增 `AgentKind::Terminal`，label 支持 `terminal` / `shell` / `vibe-terminal` / `capture-shell`。
- `crates/vb-daemon/src/lib.rs`
  - `Terminal` 发给板端的 agent meta kind 暂用 `0xFF`，不要求板端立刻新增 UI 文案。
  - passive discovery 默认禁用；`VIBE_BRIDGE_PASSIVE_DISCOVERY=1` 才允许旧 transcript fallback 进板端 UI。
  - passive focus 不再渲染 transcript 历史对话，只显示 no-live-capture 占位。
- `crates/vb-host/src/agent_discovery.rs`
  - `Terminal` 不参与 passive transcript activity/turn 解析；它的唯一可信内容来源是 ConPTY `terminal.stream`。

已验证：

- `cargo fmt --package vb-daemon --package vb-host`
- `cargo test -p vb-daemon`：34 tests passed
- `cargo test -p vb-host`：19 tests passed
- `cargo test -p vb-core`：6 tests passed
- `cargo check -p vb-daemon --target x86_64-pc-windows-gnu`
- `git diff --check -- crates/vb-core/src/lib.rs crates/vb-daemon/src/lib.rs crates/vb-daemon/src/main.rs crates/vb-host/src/agent_discovery.rs`
- 修复 os error 32 后重跑 `cargo test -p vb-daemon`、`cargo check -p vb-daemon --target x86_64-pc-windows-gnu`、`git diff --check -- crates/vb-daemon/src/main.rs` 通过。
- 第二次增强 CIM 进程匹配后再次重跑 `cargo fmt --package vb-daemon`、`cargo test -p vb-daemon`、`cargo check -p vb-daemon --target x86_64-pc-windows-gnu`、`git diff --check -- crates/vb-daemon/src/main.rs` 通过。
- 修复 v17/v18 偏差后重跑 `cargo fmt --package vb-daemon --package vb-host`、`cargo test -p vb-daemon`、`cargo check -p vb-daemon --target x86_64-pc-windows-gnu`、`git diff --check -- crates/vb-daemon/src/lib.rs crates/vb-daemon/src/main.rs` 通过。
- 修复已包装 profile 刷新后重跑 `cargo fmt --package vb-daemon`、`cargo test -p vb-daemon`、`cargo check -p vb-daemon --target x86_64-pc-windows-gnu`、`git diff --check -- crates/vb-daemon/src/lib.rs crates/vb-daemon/src/main.rs` 通过。

未验证：

- 未在 native Windows 运行 `install-windows --terminal-profiles`。
- 未打开真实 Windows Terminal profile 验证 `terminal-shim` 是否正确继承 profile 行为、输入、Ctrl+C、窗口关闭。
- 未上真板验证 `Terminal` sid 的 LCD replay/实时更新效果。

下一步最小闭环：

1. Windows PowerShell 在 `C:\Serein_Y\Sipeed\rv_nano\tools\vibe-bridge` 跑：
   ```powershell
   cargo run -p vb-daemon -- install-windows --terminal-profiles
   ```
   预期：如果旧 daemon 锁住安装 exe，会先打印 stop/retry warning，随后继续；最终看到 `wsl install: skipped`，`terminal: wrapped N profile(s) ...` 或 `already installed ...`。
2. 关闭旧 Windows Terminal 窗口，打开一个全新的被包装 profile，执行：
   ```powershell
   codex --version
   claude --version
   ```
   预期：agent 正常运行，不出现 wrapper 断链。
3. 在同一个终端里进入日常目录运行 `codex`。板端按 SESSION 选择 `Terminal: <profile>` / `terminal` sid，预期看到 v18 类似的真实 VT100 画面；不应再出现 delayed passive codex rows 或 v17 的 transcript fallback。
4. 若终端打不开，先恢复 `settings.json.vibe-bridge-backup.<epoch>`，不要改 `~/.local/bin/codex` / `~/.local/bin/claude`。

禁止重走：

- 不要默认 WSL wrapper 替换 `~/.local/bin/codex` / `~/.local/bin/claude`。
- 不要把 passive transcript scan 当作 1:1 终端还原。
- 不要改每个 agent；捕获层应在 Windows Terminal profile / shell 级别。

## 2026-05-23 最新 30 秒接手

当前优先级：**先保持普通 `codex` / `claude` 启动不被干扰，再做可选的终端捕获链路**。不要再默认安装 WSL wrapper 接管 `~/.local/bin/codex` / `~/.local/bin/claude`。

最新用户实测状态：

- `rv_nano` distro 已恢复：
  - `~/.local/bin/codex -> ~/.nvm/versions/node/v22.22.2/bin/codex`
  - `~/.local/bin/claude -> ~/.local/share/claude/versions/2.1.147`
  - `codex --version` 和 `claude --version` 已通过。
- `slam` distro 用户后续反馈：`codex` 已恢复，`claude` 也已恢复。
- Windows `install-windows` 已能输出 `wsl install: skipped`，这就是当前正确默认行为。
- Windows daemon 日志尾部已识别 HID：`vid_359f&pid_2120&mi_04`，板端当前只有一个 session，没有继续刷到 256。
- 用户新开普通 `codex` 没被识别，这是预期风险：撤掉 wrapper 后，Codex 没 hook，也没有 PTY/ConPTY 捕获，passive transcript scan 不能保证实时发现，更不能 1:1 镜像终端。

当前策略结论：

- `vibe-keyboard` 验证充分的是 hook/session/permission/state machine，不是任意已打开终端的 VT100 1:1 画面捕获。
- **纯被动监控无法保证“终端显示什么，板端一模一样显示”。** 要做到 1:1，必须拿到 PTY/ConPTY 原始字节流。
- 不要再走“默认 wrapper 替换 CLI”路线；它已造成 `codex`/`claude` 打不开，和用户“不干扰正常启动”的目标冲突。
- 推荐下一步是做一个显式入口，例如 `vibe-terminal` / `capture-shell`：daemon 用 ConPTY 启动整个 WSL shell，用户在该 shell 里正常运行 `codex` / `claude`。这样 agent 二进制不被替换，但从这个 shell 产生的终端输出可 1:1 发给板端。

下一步最小闭环：

1. 确认并保持 WSL wrapper 只作为 opt-in 路径：`install-windows` 默认只安装 Windows daemon + Windows shim，不改 WSL home。
2. 新增/整理一个显式 `capture-shell` 工作流：启动 Windows daemon 后，开一个被 ConPTY 捕获的 WSL shell。
3. 在该 shell 内运行 `codex`，板端选择该 sid 后验证 VT100 replay 是否接近“本地终端显示什么，板端显示什么”。
4. Claude 权限审批单独沿用 hook：hook 负责 `PreToolUse` allow/deny，终端镜像由 capture shell 负责，两个链路不要混在 wrapper 里。

禁止重走：

- 不要默认改 `~/.local/bin/codex` / `~/.local/bin/claude`。
- 不要把 passive transcript scan 当成实时终端镜像。
- 不要把“板端只有 session 摘要/turn 文本”说成“终端还原”。
- 不要在没拿到 Windows native daemon 日志和板端反馈前宣称 M4.3 完成。

## 2026-05-23 止血更新：WSL wrapper 改为显式启用

- 事故补充：本轮恢复后，`rv_nano` 的坏 wrapper 已保留为 `~/.local/bin/codex.vibe-bridge-wrapper-broken` 和 `~/.local/share/claude/versions/2.1.148.vibe-bridge-wrapper-broken`；`~/.local/share/vibe-bridge/real-bin/{codex,claude}` 已指向真实入口。`slam` 也已由用户恢复。

- 事故现象：WSL `codex` 被安装成 wrapper 后执行 `/mnt/c/Users/Administrator/AppData/Local/vibe-bridge/bin/vb-daemon.exe agent-shim codex`，但当前 WSL 只挂载 `C:\Serein_Y\Sipeed` 到 `/home/rv_nano/Sipeed`，没有可用的 `/mnt/c/Users/...`，导致 `codex` 打不开。
- 已恢复现场：`~/.local/bin/codex` 当前是官方 Node 入口软链，`codex --version` 通过；坏 wrapper 保留为 `~/.local/bin/codex.vibe-bridge-wrapper-broken` 便于审计。
- 本轮代码改动：`crates/vb-daemon/src/main.rs` 将 `install-windows` 默认改为不安装 WSL wrapper；只有显式 `--wsl` 或 `--wsl-distro NAME` 才会修改 WSL `~/.local/bin/{codex,claude}`。
- 本轮 daemon 改动：`crates/vb-daemon/src/lib.rs` 增加 `from_hook` 来源标记；passive prune 不再删除 hook/launch 注册的 agent，也不删除尚未拿到 board sid 的 pending passive agent，避免 HID 离线或 discovery snapshot 抖动时重复申请 `REQUEST_SESSION` 把板端 sid 刷到 256。
- 本地配置清理：`~/.claude/settings.json` 已去掉安装副本 hook，只保留仓库 `adapters/claude-code-hook/index.js` 一套；`~/.bashrc` 已去掉旧 `vibe-bridge WSL wrappers` PATH block。
- 已验证：`cargo fmt --package vb-daemon --check`、`cargo test -p vb-daemon`、`cargo check -p vb-daemon --target x86_64-pc-windows-gnu`、`git diff --check -- crates/vb-daemon/src/lib.rs crates/vb-daemon/src/main.rs`、`codex --version`。
- 下一步：在 Windows 重新构建并运行 `cargo run -p vb-daemon -- install-windows`；预期输出 `wsl install: skipped`。随后重启 Windows daemon，再看 `%LOCALAPPDATA%\vibe-bridge\vb-daemon.log` 是否停止 `registered 1` / `pruned 1` 循环。HID 日志里的 `no Vibe HID 359f:2120 device found` 仍需确认板端 USB/HID 是否在线。

## 当前在做什么

**M4.3 真机端到端**：`hook → daemon → HID → 板端 PERM → CONFIRM → daemon → hook → claude 工具放行`。

2026-05-22 更新：用户明确目标是“还原”，不是摘要/美化。当前主线改为恢复 `terminal.stream` 的真实 VT100 语义：上位机捕获到的 PTY/ConPTY bytes 必须按 sid 缓存，板端 focus 时 replay `clear + buffer`，focus 后实时转发。被动发现仍可列出裸开的 agent，但没有 PTY/ConPTY 捕获的裸进程不能无侵入 1:1 还原。

2026-05-22 补充：用户进一步明确 Windows 上应“尽量什么都不执行，开机就在跑，普通 agent 自动 hook”。已在 `crates/vb-daemon/src/main.rs` 增加 `install-windows` 和 `agent-shim`：安装时把 `vb-daemon.exe` 复制到 `%LOCALAPPDATA%\vibe-bridge\bin`，写入 Startup 的 `vibe-bridge-daemon.cmd`，并生成 Windows `codex.cmd`/`claude.cmd` shim 放到用户 PATH。2026-05-22 再扩展为“全机安装”：默认 `wsl.exe -l -q` 枚举 WSL distro（也可重复传 `--wsl-distro <NAME>` 指定），在每个 distro 内安装 `~/.local/bin/codex`/`claude` wrapper，保存真实 CLI 到 `~/.local/share/vibe-bridge/real-bin/`，追加 shell PATH block，复制 Claude hook 到 `~/.local/share/vibe-bridge/claude-code-hook/index.js` 并合并 `~/.claude/settings.json`。日常使用应是 Windows 或 WSL 新终端直接 `codex`/`claude`，shim 确保 daemon 后用 ConPTY 捕获真实 VT100。已打开但未经过 wrapper 的 agent 仍靠 passive discovery 列出，只能 fallback 显示；开机自启后新开的 agent 才有 1:1 捕获。`launch` 仍可调试，但不应再让用户手动记这条命令。 本轮修复 `install-windows` 真机反馈：过滤 `docker-desktop`/`docker-desktop-data`，不再在 WSL 内调用 `wslpath` 转换 daemon exe，而是在 Windows 侧转换为 `/mnt/<drive>/...`；安装结束后立即后台启动 daemon 并打印 `daemon now`，不再等下次开机；WSL wrapper 找不到 real CLI 时直接报错不递归；daemon 每 2s 发送 broadcast `SessionHeartbeat` 作为 host-level 心跳基础。

代码全部完成（含本轮 6 行关键修），**未亲验**。卡点已定位、已修，**需要你手动在全新 claude session 跑一次**才能闭环。

- daemon 跑在 Windows native (`cargo run -p vb-daemon -- serve-hid 127.0.0.1:8765 auto`)
- hook 装在 `/home/rv_nano/.claude/settings.json`（**只 rv_nano distro，slam 没装**）
- WSL2 是 mirror 模式，hook 127.0.0.1 直通 Windows daemon
- 端口 8765
- 板端 SHA 未变：`aikb_hid_input=63ffeee6…`、`aikb_lcd_ui=c178b992…`

## 已经试过的方案和结果（含失败的）

### 本 session 的改动（4 个真 bug + 1 个 UX）

0. **恢复真实 VT100 buffer/replay（2026-05-22）**：
   - `crates/vb-daemon/src/lib.rs` 给 `RegisteredAgent` 增加 64 KiB raw VT100 ring buffer。
   - `terminal.stream` 现在先写入 per-agent buffer；只有该 sid 已被板端 focus 时才实时发 HID。
   - `SESSION_FOCUS` 时优先发送 `ESC[2J ESC[H + terminal_buffer`，这才是“终端显示什么，板端也显示什么”的还原路径。
   - 如果 launched session 尚无 terminal bytes，只清屏；如果 passive session 无 terminal bytes，才保留现有摘要 fallback。
   - `crates/vb-daemon/src/main.rs` 把 Windows `launch/start` 默认 ConPTY 尺寸从 `120x30` 改为板端默认 `78x15`，避免 Codex/Claude 按大终端布局后板端挤压。
   - 已验证：`rustfmt --check crates/vb-daemon/src/lib.rs crates/vb-daemon/src/main.rs`、`cargo test -p vb-daemon`、`cargo check -p vb-daemon --target x86_64-pc-windows-gnu`、`cargo test --workspace`、`git diff --check -- crates/vb-daemon/src/lib.rs crates/vb-daemon/src/main.rs`。
   - 未验证：Windows native 真 HID + 真板 LCD。`cargo fmt --all --check` 仍因既有 `crates/vb-host/src/agent_discovery.rs` 一行格式失败，本轮没碰该文件。

1. **Discovery filter 改成 active-process-only + (kind,cwd) 去重**：
   - `crates/vb-host/src/agent_discovery.rs::filter_sessions_by_active_counts`
   - 之前 multi-signal union (active + status_active + recently_modified) 引入 Typora 死会话不消失 + claude 状态错乱
   - 现在严格：必须 active 进程匹配；空时回退 `fallback_recent_active_candidates`
   - **active_slots 改成 `counts.into_keys().collect()`**（关键），每 (kind,cwd) 最多 1 槽，解决 codex 父+子双进程 → 重复 SID + 跨 tick 不稳定
   - prefix-match cwd 处理 Claude 把 transcript cwd 改成当前 `cd` 路径的情况

2. **wsl.exe -d <distro> sh -lc <script> 静默崩**（重大 bug）：
   - 症状：`[scan] ... 0 agent process(es)` 三个 distro 全 0
   - 根因：多行 POSIX 脚本经 Windows argv → wsl.exe arg parser → bash，中间任一层把内嵌引号吃了
   - 修：**base64 编码脚本**，`wsl.exe -d <distro> -- sh -lc "echo <B64> | base64 -d | sh"`
   - 自带 std 实现 `base64_encode_bytes`，零依赖

3. **agent_id 命名不对齐**（M4.3 的真正卡点）：
   - 症状：hook 注册的 agent_id 是 `claude-<UUID>`（带前缀），passive scan 注册的是 `<UUID>`（无前缀）
   - 后果：daemon `registered` 里出现两条 RegisteredAgent，板端发两个 SID，CONFIRM 永远不影响 hook 等的那条
   - 修：`adapters/claude-code-hook/index.js::resolveAgentId` 改成 `return input.session_id || ...`，去掉 `claude-` 前缀
   - **必须 hook 端和 vb-host 端保持同一种 agent_id 命名**

4. **session.abort 不清 registered**：
   - 之前只 `status = Done`，agent 留在 `registered`，板端心跳照发
   - 修：直接 `registered.remove()` + drain `pending_board_sessions`
   - 配合 `prune_missing_passive_sessions`：discovery 每 1.5s 比快照，掉队 passive 直接 unregister，心跳停发，板端按心跳 fade-then-drop

5. **板端 terminal view 渲染**（v8.png 对齐）：
   - 加 `render_passive_session_view`：78 列（板端 LCD 实测 `g_cols = (UI_W=960 - 2*8) / cell_w=12 = 78`）、8-color SGR（板端**不支持** `\x1b[38;5;Nm` 256-color，只支持 `30-47/90-107/38;2;R;G;B`）、纯 ASCII separators
   - 旧状态：launched (`from_launch=true`) 焦点回放只清屏让 ConPTY 重绘；passive 走 `render_passive_session_view`
   - 2026-05-22 已改：如果有真实 terminal buffer，focus 时 replay `clear + buffer`；没有 buffer 才走旧 fallback
   - turn.append / permission.request 时如果板端正在聚焦该 SID，立刻 push 新一帧（实时更新）

### 没解决但放下的

- **v7/v15 风格还差**：用户嫌"挤、字小、不像 v7"。本质是 board 端 LCD 小 + 我只调到一个 baseline，没 polish pixel-level layout。下一轮调 layout / 试 16x32 preset / 加 powerline 替代字符可能改善
- **slam distro 那边没装 hooks**：只 rv_nano（Ubuntu-22.04）user 的 `~/.claude/settings.json` 加了 hook。slam 那边自己日常用的 claude 触发不到 PERM
- **codex 没 hook 通道**：codex 不像 Claude Code 有 PreToolUse hook。codex 走 transcript scan，被动显示，没有反向控制

### 失败/被否决的方案

- **第 1 次 `start` 子命令**（包 ConPTY 拉 claude，做 v7 镜像）：用户否决，要"被动 hook 已开着的 claude"，不要新窗口
- **多次在 strict / loose filter 之间反复**：浪费 3 轮
- **256-color SGR**：板端不支持，silently dropped。要 8-color 或 24-bit RGB
- **Unicode powerline 字符 (■ › ✱ ⚙ ⚠)**：不知道板端 SarasaMonoSC 在 12x24 cell 上能不能正常渲染，改 ASCII 保险

## 下一步计划（3-5条 actionable）

1. **真机验 VT100 还原闭环**：
   ```powershell
   cd C:\Serein_Y\Sipeed\rv_nano\tools\vibe-bridge
   cargo run -p vb-daemon -- serve-hid 127.0.0.1:8765 auto
   ```
   新窗口启动一个被捕获的 session：
   ```powershell
   cargo run -p vb-daemon -- launch --kind codex -- codex --sandbox workspace-write --ask-for-approval on-request --add-dir $env:USERPROFILE\Sipeed
   ```
   或一体化：
   ```powershell
   cargo run -p vb-daemon -- start --kind codex -- codex --sandbox workspace-write --ask-for-approval on-request --add-dir $env:USERPROFILE\Sipeed
   ```
   板端按 SESSION 选择该 sid 后按 CONFIRM，预期看到 Codex 真实 VT100 TUI 的最近画面；focus 后新输出实时更新。

2. **真机验 M4.3 一次性闭环**（审批回传）：
   ```powershell
   cd C:\Serein_Y\Sipeed\rv_nano\tools\vibe-bridge
   $env:VIBE_BRIDGE_SCAN_DEBUG = "1"
   cargo run -p vb-daemon -- serve-hid 127.0.0.1:8765 auto
   ```
   新窗口：`wsl -d Ubuntu-22.04` → `cd /tmp` → `claude` → 在 claude 内 `> run \`date\``。
   期望 daemon stderr：
   ```
   [register] claude/<UUID> from_launch=false (existing=false)   ← hook 第一次
   [register] claude/<UUID> from_launch=false (existing=true)    ← UserPromptSubmit
   [register] claude/<UUID> from_launch=false (existing=true)    ← PreToolUse
   ```
   `existing=true` 是 hook & passive 命名对齐的证明。板端 SID 看到 `! PERM #...` 行，按 CONFIRM → 工具放行；再触发一次按 KEY0 → 工具 deny。
   - 通了：M4.3 关闭。
   - 不通：把完整 daemon stderr + 板端表现贴回来，先看 `[register]` 行 existing 是否 true。

3. **slam distro 也装 hook**（让 slam 那边的 claude 也能审批）：
   ```powershell
   wsl -d Ubuntu bash -lc 'cat > ~/.claude/settings.json << EOF
   ... 跟 rv_nano 的一样 ...
   EOF'
   ```
   注意 hook 命令里的 `/home/...` 路径要换成 slam 视角的（如果 vibe-bridge 在 slam 也有视角），或者用 `/mnt/c/Serein_Y/...`。

4. **板端 CONFIRM 自动 2s 弹回**（Task #10）：
   - 板端 C 代码改：`aikb_lcd_ui.c` 加 `last_confirm_ms`，进 confirm 子屏后 2s 自动回 picker
   - 跨端协议无需变更

5. **commit 本 session 改动**：当前 git status 列了多处修改+多个新增 crate（vb-agent/vb-platform/vb-protocol/vb-transport/conpty.rs）。实测前不建议 commit；实测后按“恢复 VT100 replay + discovery/hook 对齐 + M4.3”分组。

## 关键文件路径（相对路径，一行一个）

HANDOFF.md
adapters/claude-code-hook/index.js
adapters/claude-code-hook/README.md
crates/vb-daemon/src/lib.rs
crates/vb-daemon/src/main.rs
crates/vb-daemon/src/conpty.rs
crates/vb-host/src/agent_discovery.rs
crates/vb-host/src/lib.rs
crates/vb-platform/src/lib.rs
crates/vb-protocol/src/payloads.rs
crates/vb-transport/src/windows.rs
docs/request.md
../../../../AIKB/HANDOFF.md
../../../../AIKB/LicheeRV-Nano-Build/middleware/v2/sample/aikb_hid_input/aikb_hid_input.c
../../../../AIKB/LicheeRV-Nano-Build/middleware/v2/sample/aikb_lcd_ui/aikb_lcd_ui.c
../../../../Sipeed/rv_nano/tools/Vibe_Bridge/README.md
images/logs/v7.png
images/logs/v15.png

## 还没搞清楚的问题

- **板端 LCD 实际渲染**：78×15 是默认 preset 算出来的，**没在板端 dump 帧验证实际显示效果**。用户 v15 反馈"挤、字小、丑"，可能是 cell preset 不对、或 SarasaMonoSC 字距、或 ANSI color 实际渲染颜色。要拿到板端真实截图反向调整。
- **agent_id 对齐是否真生效**：本 session 没真跑过 hook → daemon → board → CONFIRM 完整一次。`[register] existing=true` 的预期没被实测验证。可能还有 hook 端 input.session_id 拿不到的情况（claude code 可能在某些 hook 事件不传这个字段）。
- **codex 反向控制**：codex 有没有类似 Claude Code 的 hook 机制？没查。如果有，可以一并接入。
- **slam distro 的 hooks**：没装。slam 那边 3 个 codex + 用户自己的 claude 都没有 PreToolUse 反向。要做 (ii) 必须装。
- **CONFIRM 子屏的板端语义**：用户反馈"按 CONFIRM 跳到 confirm 子屏后应该 2s 自动弹回上一屏"。这是 aikb_lcd_ui.c 板端状态机改动，没动。
- **start 子命令的命运**：本 session 新增了 `vb-daemon start --` 用来打 ConPTY 镜像（v7-fidelity）。用户后来转向 passive-only 路径。`start` 当前**保留但用户不用**，下次决定 keep / remove。
- **Windows/真板 VT100 replay 未实测**：Rust 单测已证明 `terminal.stream` 会缓存并在 `SESSION_FOCUS` 后 replay，但还没在 Windows native + 真实 HID + LCD 上确认 Codex/Claude TUI 是否达到“本地终端显示什么，板端显示什么”。
- **codex 父+子进程 dedup** 后会不会丢真实多开**：用户在同 cwd 同时跑 2 个 codex 进程组的情况罕见，当前 (kind, cwd) 去重只保留 1 个 SID。如果真有人这么用，体验回归。
- **C_context/MEMORY.md 状态**：git status 显示 dirty，本 session 没看也没碰。

## 2026-05-24 Windows Terminal 身份反馈修正

- 现象：用户在 Windows Terminal PowerShell 中输入 `Write-Host VIBE_BOARD_E2E` / `1212`，板端几乎实时同步，但 LCD 底部仍显示 `Claude`，看起来像另一个窗口。
- 判断：不是 VT100 stream 串 sid。`aikb_hid_input` 只在 `sid == g_active_sid` 时转发 `CMD_VT100_STREAM`；截图内容和 Windows 终端一致，说明焦点门控已对上。误差在“会话身份反馈”。
- host 修正：`vb-daemon launch` 对 `kind=terminal` 的 display name 追加当前 shim pid，例如 `Terminal: Windows PowerShell #26372`，避免多个同名 profile 在 picker 里不可区分。
- 板端配合：`aikb_hid_input` 把 `CMD_REQUEST_SESSION` hint 透传为 `session N hint TEXT`；`aikb_lcd_ui` 保存 hint，SESSION picker 优先显示 hint，terminal 状态条第三段从硬编码 `Claude` 改为动态 `Terminal #sid` / `Claude #sid` / `Codex #sid`。
- 已验证：`cargo fmt --package vb-daemon`；`cargo check -p vb-daemon --target x86_64-pc-windows-gnu`；`cargo test -p vb-daemon`；板端 RISC-V 交叉编译 `aikb_lcd_ui` / `aikb_hid_input` 通过并 strip。
- 已同步板端产物：两个 RISC-V stripped binary 已同步到 `buildroot/board/cvitek/SG200X/overlay/mnt/system/usr/bin/`、`buildroot/output/target/mnt/system/usr/bin/`、`install/soc_sg2002_licheervnano_sd/rootfs/mnt/system/usr/bin/`。
- 待用户验证：在板端实际替换/重启服务后，新 Windows Terminal session 应在 picker 看到带 pid 的 `Terminal: ... #pid`，terminal 底部不再显示 `Claude`，而显示当前 active sid。

## 2026-05-24 agent-aware 主链路推进

- 主要矛盾：Windows Terminal profile 捕获已经能显示普通 shell，但如果 WindowsTerminal 进程早于 `install-windows` 启动，它不一定继承用户 PATH 更新；用户在新 tab/window 里输入 `codex`/`claude` 可能绕过 `codex.cmd` / `claude.cmd` shim，导致板端只有 `Terminal` session，没有 agent session。
- 控制动作：`terminal-shim` 进入 `run_launch(kind=terminal)` 时，若当前 exe 目录存在 `codex.cmd` 或 `claude.cmd`，临时把该目录 prepend 到子 shell 的 PATH，并向子 shell 注入 `VIBE_BRIDGE_DAEMON=<addr>`。这样被捕获的 PowerShell 即使没有刷新系统 PATH，也会优先走 agent shim。
- 预期产品行为：新开被捕获 Windows PowerShell 后直接输入 `codex` 或 `claude`，应出现普通 `Terminal: ... #pid` session，同时 agent shim 再注册 `codex` / `claude` session；用户可在 SESSION picker 选 agent sid 看 agent TUI。
- 已验证：`cargo fmt --package vb-daemon`；`cargo check -p vb-daemon --target x86_64-pc-windows-gnu`；`cargo test -p vb-daemon`；`git diff --check -- crates/vb-daemon/src/main.rs HANDOFF.md`。
- 待 Windows 实测：安装后不用重启 WindowsTerminal 主进程，新开被捕获 PowerShell，执行 `where.exe codex` 应优先看到 `%LOCALAPPDATA%\vibe-bridge\bin\codex.cmd`；执行 `codex --version` 或进入交互后，日志应出现 `[launch] ... kind=codex ...` 和 `[register] codex/launch-...`。
