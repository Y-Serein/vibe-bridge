# vibe-bridge Memory

## 2026-05-27 AIKB 板端 UI / sleep / 900M rootfs 复盘

### 当前状态记忆

- 本轮主线是板端，不是上位机：AIKB UI 统一、boot 动画删除、sleep 动画接入、rootfs 缩到 900M，并补项目定义文档。
- 板端仓库是 `/home/rv_nano/AIKB/LicheeRV-Nano-Build`；项目文档和 memory 放在 `/home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge/C_context`。
- 用户已经烧录验证过上一版“挺正常”，但本轮最终改动尚未由用户重新 `pack_rootfs && pack_burn_image`、烧录和上板验证。
- 用户明确保留打包镜像权限：agent 不运行 `pack_rootfs && pack_burn_image`，只把代码/配置做到可编译。
- 目标镜像不是越小越好，而是在稳定性有余量的前提下，从约 1.6G 收敛到 900M rootfs / 约 916MiB burn image，满足约 1GB TF / SD NAND 方向。

### 用户偏好

- 默认中文，简洁但要有证据；结论要区分“已验证”和“未验证”。
- 用户希望按“目标→状态→误差→控制动作→反馈→修正→验证→沉淀”闭环推进，尤其在嵌入式、镜像、硬件相关任务中。
- 用户反复强调不要大改架构。UI 优化要尽量保持已有实现和产品行为稳定。
- 用户不接受 UI 标题自动替换：按键按下后左上角应显示对应主题标题，不应几秒后从 `VOICE` 变成 `LISTEN`、从 `RUN` 变成 `UPDATING`。
- 用户更在意产品语义一致性：`RUN` 页面不能显示 OTA、updating、progress 这类旧语义。
- 用户要求文档放在 `Sipeed/rv_nano/tools/vibe-bridge/C_context`，不要放 AIKB。
- 用户说“内存小一点”时要先确认语义；这次指 TF / SD NAND 存储容量，不是 256MB DDR。
- 用户需要 3 天后 30 秒接续，HANDOFF 顶部必须写清目标、状态、误差、动作、验证、下一步和不要重复的坑。

### 从错误里学到的最佳实践

- 不要越权运行用户明确保留的高风险命令。`pack_rootfs && pack_burn_image` 会生成生产镜像，必须由用户执行。
- 不要把“配置改到 900M”说成“已生成 900M 新镜像”。没有 pack 就没有新镜像。
- 缩镜像时先区分三件事：分区 XML 决定 burn image 布局，Buildroot ext4 size 决定 rootfs 文件系统大小，post-build prune 决定 rootfs 内容大小。
- 1GB 介质要按十进制 1GB 预留空间，960M rootfs 太贴边；900M rootfs 更稳。
- 动画文件可能用 mmap 播放，占用存储不等于常驻 DDR；删除 boot 动画主要缩镜像，不应宣称显著降低运行内存。
- 屏幕测试版本只能临时用于显示验证，用户测试完必须恢复正常系统启动。
- 资源裁剪必须基于引用审计：`/mnt/system/auto.sh`、板端 C 程序、overlay、target/install rootfs 之间容易漂移。

### 项目关键约束和坑

- 板端可提交源码/配置/overlay/sample binary；生成产物、`.d` 依赖文件、rootfs 镜像不要默认提交。
- `buildroot/board/cvitek/SG200X/overlay/mnt/system/auto.sh` 是板端启动体验关键入口，boot/sleep/UI 参数都从这里接入。
- `aikb_lcd_ui` 负责本地 UI、pet/terminal/session/picker/sleep；`aikb_hid_input` 负责 HID/session/key/event 桥接。不要把标题/UI 问题误判为 host session 问题。
- boot 动画 `vedio_start.akim` 已被删除；sleep 动画 `vedio_sleep.akim` 是保留并启用的资源。
- 非 session 页面才允许 idle sleep；terminal/session picker 页面不能被 sleep 动画盖住。
- rootfs 900M 依赖两处一致：`partition_sd.xml` 的 `ROOTFS=921600KB` 和 Buildroot `BR2_TARGET_ROOTFS_EXT2_SIZE="900M"`。
- AIKB 当前运行链路是 BusyBox/C 程序为主；Python/Qt/OpenCV/ffmpeg/gdb/vim/demo model 是本轮裁剪对象，但未来如果引入依赖必须重新审计。

### 下次一次性达到目标的提示词

```text
接手 AIKB 板端 UI / rootfs 优化，按“目标→状态→误差→控制动作→反馈→修正→验证→沉淀”闭环推进。先读 /home/rv_nano/AIKB/AGENTS.md、CLAUDE.md、HANDOFF.md、C_context/KNOWN_FAILURES.md，以及 /home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge/C_context/MEMORY.md，并运行 /home/rv_nano/Sipeed/T_tools/agent_preflight.py --project AIKB 或项目规则指定的 preflight。

约束：板端是重点；尽量不改架构；不要运行 pack_rootfs && pack_burn_image，这一步由我来跑；不要把文档放 AIKB，文档和复盘放到 Sipeed/rv_nano/tools/vibe-bridge/C_context。

当前产品要求：UI 标题按键后保持对应主题，不要几秒后自动变成内部状态名；RUN 页面只显示 running 语义，不显示 OTA/updating/progress；删除 boot 动画；非 session 页面 3 分钟无按键进入 sleep；镜像目标 900M rootfs，兼容约 1GB TF / SD NAND，但要保留稳定余量。

验证要求：只做静态检查、最小编译或语法检查；明确告诉我哪些未上板验证。结束时更新 HANDOFF 顶部 30 秒接续、C_context/MEMORY.md，并把流程/坑沉淀进 skill。
```

### 已沉淀为 skill

- 新增项目 skill：`C_context/skills/aikb-board-control-loop/SKILL.md`。
- 下次处理 AIKB 板端 UI、AKIM、sleep、rootfs、镜像大小、LicheeRV-Nano-Build 打包边界时应优先使用。

## 2026-05-26 上位机产品收尾复盘：native WSL 与安装包边界

### 当前状态记忆

- direct Ubuntu -> `codex` 已由用户实测确认效果较好，这是当前产品链路里最接近目标的路径。
- 上位机本轮重点是 Windows Terminal 中 `wsl -> codex` 的 native 体验：host 终端样式、颜色、回滚不能被捕获层破坏。
- 当前正确策略：普通 `wsl` 保持 native passthrough，进入 WSL 后由 shell-integration 的 `codex` / `claude` shim 捕获。不要继续用双层 ConPTY/transient shim 硬包 `wsl`。
- 新 `VibeBridgeSetup.exe` 尚未重建。WSL 环境缺 `x86_64-w64-mingw32-gcc`，也没有可用 `powershell.exe` / `cargo.exe`。当前 `D_deliverables/windows/VibeBridgeSetup.exe` 是旧包，不能用于验证本轮 host 修正。
- 板端 SESSION/scrollback 细节已沉淀到 `/home/rv_nano/AIKB/HANDOFF.md` 和 `/home/rv_nano/AIKB/C_context/MEMORY.md`，不要在上位机 memory 中展开维护。

### 用户偏好

- 用户要成品产品感，不要 demo 感：类似驱动/Typora 安装体验，安装、修复、卸载清晰，后台常驻，卸载后不影响系统。
- 用户非常在意“原生态终端体验”。如果捕获策略让 Windows Terminal 回滚、样式、颜色、交互不像原生，即使板端能看到，也不是合格默认方案。
- 用户不接受“只能通过我指定入口打开”的局限。direct Ubuntu、Windows Terminal 里输入 `wsl`、再运行 `codex`，都应尽量覆盖。
- 用户反复要求不要破坏正常 `codex` / `claude`。不要覆盖 `~/.local/bin/codex` / `~/.local/bin/claude`；shell integration 应只前置自己的 shim 目录，并能卸载恢复。
- 用户希望我能反驳，但必须给证据和替代方案利弊。本轮应明确反驳“继续包一层 terminal-shim 就能修好 wsl 样式”：它会和 native scrollback/style 目标冲突。
- 用户希望我主动说明未验证项。尤其不能把“代码通过 check”说成“安装包已可用”，也不能把旧 exe 当新包。
- 用户希望交接文件顶部 30 秒可接续：目标、当前状态、误差、控制动作、验证、未完成、下一步、不要重复的坑都要写明。

### 从错误里学到的最佳实践

- 不要把“能捕获”误当成“产品体验好”。捕获层如果改坏终端原始样式/回滚，就要降级为显式入口，而不是默认入口。
- 对 `wsl -> codex`，更稳的默认策略是：`wsl.cmd` passthrough 到 `wsl.exe`；进入 WSL 后由 shell-integration 捕获 agent。不要再默认 nested ConPTY 捕获整个 `wsl`。
- 修安装器策略后，必须同步修脚本输出文案。否则用户会按旧“wraps Windows Terminal profiles”理解产品行为。
- 生成/验证 Windows 安装包必须在 native Windows 或有 mingw linker 的环境完成。WSL 里 `cargo check --target x86_64-pc-windows-gnu` 通过不等于能 release link 出 exe。
- 修改产品默认策略后，必须配套更新 setup 输出文案、Start Menu 行为和卸载/修复路径，避免用户按旧语义测试。

### 项目关键约束和坑

- Host repo：`/home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge`。
- 用户实际 Windows 项目目录：`C:\Serein_Y\Sipeed\rv_nano\tools\vibe-bridge`。agent 侧不要直接假设可通过 Windows 路径访问，优先 WSL 路径。
- 产品默认不应 wrapper Windows Terminal profiles；需要显式 captured WSL shortcut 时，放在 `vibe-bridge` 子菜单，不覆盖 direct Ubuntu/WSL 入口。
- Windows 安装/修复应恢复 native Windows Terminal profile；卸载也必须恢复 terminal/profile/startup/shims/shell integration。
- WSL shell integration 当前应安装到 `~/.local/share/vibe-bridge/shell-integration/bin/{codex,claude}`，并通过 marker block 管理 PATH。不要移动真实 CLI，不要写坏 `real-bin`。
- Board-assigned SID 仍是权威；host 不能自造最终 sid。
- 当前未完成项：Windows 新 setup exe 未重建；native Windows install/repair 未实测。

### 下次一次性达到目标的提示词

```text
接手 vibe-bridge 产品收尾，按“目标→状态→误差→控制动作→反馈→修正→验证→沉淀”闭环推进。先读 AGENTS.md、HANDOFF.md、C_context/MEMORY.md、C_context/KNOWN_FAILURES.md，并运行对应 agent_preflight.py。先汇报状态，再做最小改动。

当前事实：direct Ubuntu -> codex 已经比较好；不要从“完全没捕获”重新排查。当前重点是上位机产品体验一致性：Windows Terminal 中 wsl -> codex 要保持 native WSL 样式和回滚，同时 WSL shell-integration 仍能让 Codex 注册到 daemon。

策略边界：不要默认 wrapper Windows Terminal profiles；不要覆盖 ~/.local/bin/codex 或 ~/.local/bin/claude；普通 wsl.cmd 应 passthrough 到 wsl.exe，进入 WSL 后由 shell-integration 的 codex/claude shim 捕获。需要 captured WSL 的入口只能作为显式 Start Menu 子菜单入口，不要覆盖原始 Ubuntu/WSL 入口。

验证要求：host 侧跑 cargo fmt/test/check 和 Windows target check。不要把旧 D_deliverables/windows/VibeBridgeSetup.exe 当新包；如果 WSL 不能 link Windows exe，明确要求我在 native Windows PowerShell 运行 .\T_tools\build_windows_product.ps1。

结束时更新 HANDOFF.md 顶部 30 秒接续段和 C_context/MEMORY.md；列出未验证项、用户下一步命令、预期结果和失败分支。
```

### 已沉淀为上位机 skill

- 项目 skill：`C_context/skills/vibe-bridge-control-loop/SKILL.md`。
- 本轮已要求补充 native WSL/product install 规则；下次处理 vibe-bridge Windows daemon、WSL shell integration、安装包收尾时应优先使用。
- 不要同步到全局 `~/.codex/skills`，除非用户明确要求；这是上位机项目级 skill。

## 2026-05-24 合作复盘：停止盲猜，沉淀板端空屏阶段

### 当前状态记忆

- 当前主线不是重新证明 Windows Terminal capture 是否存在；host 侧已经实测到捕获 PowerShell、捕获 WSL、临时 WSL shim、`codex --version` 成功。
- 用户打开 Codex 后，板端已经出现过 session，但进入后只闪现约 200ms 终端内容，随后变成黑色空屏，只有 `_` 光标闪烁。
- 这说明问题已经进入“有 session / 有首帧 / 后续显示状态丢失”的阶段，不能再用“没有捕获入口”解释全部现象。
- 最新 board `aikb_lcd_ui` hash 记忆：`f79b8a426326a34407c530456f884b9f9c493980c7a6cb6f59554f94b1bd329f`。
- 下一步应先加 host/board 计数器，区分 stream、sid、parser、clear、viewport，不应继续靠视觉猜测 ANSI parser。

### 用户偏好

- 用户要的是产品行为，不是 demo 行为：安装插件后不能改变平时使用方式，不能要求 WSL 用户先去 PowerShell 里跑 `codex`。
- 用户接受“已经打开且不是通过捕获入口启动的 agent 不能可靠 1:1 接管”，但要求新打开的正常入口必须做好。
- 用户强调 UX 优先级很高。功能实现不能以重复输入、跳行、卡死、杀终端、污染 PATH 或破坏 CLI 为代价。
- 用户希望我可以直接反驳，但要基于证据和架构边界，不要用模糊措辞拖延。
- 用户不想被当成探索脚本执行器。需要用户验证时，必须给完整命令、运行位置、预期结果和失败分支。
- 用户要求每次工作后给进度百分比，并说明这个百分比的 100% 是什么。
- 用户问“哪里改了”时，要给具体文件、关键词、line anchor、diff 摘要或 hash，不能口头宣称。
- 用户强烈反感幻觉式汇报：没有实际改代码、没有运行验证、没有证据时必须直接说明。

### 从错误里学到的最佳实践

- 不要把“结构上接近”说成“快完成”。只要板端 TUI 仍然闪后空屏，就不能按 98% 表达。
- 不要建议杀 Windows Terminal。用户的 Codex 可能就运行在那个终端里，这会直接中断合作。
- 不要在捕获终端里执行会重启/替换当前 daemon 的安装命令；安装应从未被捕获的 `powershell.exe -NoLogo -NoProfile` 运行。
- 不要混淆 shell 语境。`$env:...` 是 PowerShell；`echo $VAR` 是 WSL bash；`where.exe` 是 Windows；`which` 是 Linux/WSL。
- 不要把“黑色空屏”描述成“白屏”。视觉事实要按用户原话记录。
- 不要让用户反复烧录没有新诊断能力的固件。若现象没有可区分证据，先加计数器再让用户验证。
- 不要继续盲补 ANSI parser。两次视觉修复失败后必须转为仪表化诊断。
- 不要用 `codex --version` 作为最终目标。它只是验证 shim/exec 路径的低成本探针；产品目标是正常 WSL 使用方式下新开的 agent 被捕获并在板端稳定显示。
- 当用户质疑架构合理性时，要先判断“这是不是改变了用户习惯”。如果改变习惯，即使技术上可行，也不能作为默认产品方案。

### 项目关键约束和坑

- Native Windows 是验证主路径；不要要求用户回到 WSL hidraw 路径验证产品能力。
- Windows Terminal profile capture 是当前主入口；直开未被捕获的 WSL/Ubuntu 入口不会天然有 `VIBE_BRIDGE_TERMINAL_AGENT_ID`，不能假装已覆盖。
- 临时 WSL shim 必须只对当前捕获 WSL shell 生效，不写 `.bashrc`、不改 `~/.local/bin`、不污染永久 PATH。
- Board-assigned SID 仍是权威；host 只能请求和绑定，不能自造最终 sid。
- Terminal mirror 的可信内容来源是 PTY/ConPTY VT100 bytes，不是 transcript、历史对话或摘要。
- 当前板端空屏的分层判断：
  - 没 session：查 terminal-shim/register/request/response。
  - 有 session 但无首帧：查 focus/replay。
  - 有首帧后空屏：查后续 stream、clear、parser、viewport、active sid。
- 黑色空屏但 `_` 光标闪烁，说明 UI 主循环大概率仍活着；优先查数据状态，不要先假设板端进程死锁。

### 下次一次性达到目标的提示词

```text
接手 vibe-bridge，按“目标→状态→误差→控制动作→反馈→修正→验证→沉淀”闭环工作。先读 AGENTS.md、HANDOFF.md、C_context/MEMORY.md、/home/rv_nano/Sipeed/C_context/KNOWN_FAILURES.md，并运行 /home/rv_nano/Sipeed/T_tools/agent_preflight.py --project vibe-bridge。

当前阶段：host 侧 Windows Terminal/WSL/Codex 注册链路已基本打通；用户打开 Codex 后板端有 session，但进入后终端内容闪现约 200ms，随后变成黑色空屏，只剩 `_` 光标闪烁。不要把它说成白屏，也不要再从“完全没捕获”开始排查。

目标：用户安装后不改变平时习惯；新开的 PowerShell/WSL/Ubuntu/Codex/Claude 能被捕获；板端显示真实 live TUI；不能用 transcript 摘要冒充；不能破坏用户终端体验。

下一步策略：不要继续盲补 ANSI parser。先加 host/board 诊断计数器，区分 focus replay、live stream、VT100 packet、FIFO、parser printable、clear、viewport、active sid。只有拿到分层证据后再修代码。

每次工作结束给进度百分比，并说明 100% 的定义。需要我在 Windows/板端验证时，给完整命令、运行位置、预期输出和失败分支。
```

### 已沉淀为 skill

- 项目内 skill：`C_context/skills/vibe-bridge-control-loop/SKILL.md`。
- 本轮已要求 skill 增加“板端闪后空屏必须先仪表化”的规则，避免下次继续挤牙膏式 patch。

## 2026-05-23 terminal profile capture 排查记忆与合作偏好更新

### 当前状态记忆

- 用户最新反馈：Windows `install-windows --terminal-profiles` 已显示 daemon running、startup installed、terminal profiles wrapped，但新建 Codex 后板端仍为空。
- `docs/logs.txt` 最新证据显示 daemon 已找到 HID，registration IPC 已监听，passive discovery 已禁用；但没有 `[register]` / `[board] request session` 日志。
- 结论：当前不是 Codex transcript 解析问题，也不是首要 HID 问题，而是 Windows Terminal profile 没有命中 `terminal-shim`，或 `terminal-shim` 启动后过早失败且没有日志。
- 下一步最佳控制动作：先补/查 `terminal-shim` 启动观测点，不要继续改 transcript 或 passive discovery。

### 用户偏好

- 用户要的是产品级“一次启用”，不是让用户每个目录、每个 agent、每个终端入口都重新安装或手动换启动命令。
- 用户不能接受“必须从我们指定终端入口启动工作流”作为最终产品方案；如果技术上必须接管 PTY/ConPTY，接管层应尽量下沉到 Windows Terminal profile、daemon 或系统级入口，而不是要求用户改习惯。
- 用户强调不能影响正常 `codex` / `claude` 等 agent 功能；任何 wrapper、PATH、CLI 替换都必须默认关闭、显式 opt-in、可恢复。
- 用户要板端原样显示终端数据。历史 transcript、摘要卡片、turn append、美化后的对话框都不能冒充真实终端。
- 用户允许反驳，但反驳要直接说清事实边界和替代方案利弊。例如必须明确说：纯被动扫描和网络消息网关都不能还原终端 TUI 画面。
- 用户愿意配合 Windows/真板跑命令，但需要完整命令、预期输出、失败分支；不要让用户跑没有区分度的探索性命令。
- 用户偏好闭环表达：目标 -> 状态 -> 误差 -> 控制动作 -> 反馈 -> 修正 -> 验证 -> 沉淀。

### 从错误里学到的最佳实践

- 不要把“能发现 Codex transcript”说成“能还原终端”。passive discovery 最多能列出会话和摘要，不能重建 TUI 的光标、增量 repaint、选择态、布局和控制序列。
- 不要默认替换 `~/.local/bin/codex` / `~/.local/bin/claude`。这类改动一旦出错，会直接破坏用户正常工作流，比板端不显示更严重。
- Windows Terminal capture 的关键证据不是板端是否出现 Codex，而是 daemon log 是否出现 `[terminal-shim] start`、`[register] terminal/...`、`[board] request session ...`、`[board] session response ...`。缺哪个就定位哪一段。
- 日志必须覆盖进程最早期。`terminal-shim` 如果在注册前失败，daemon 不会天然知道它启动过；必须让 shim 入口自己写诊断日志。
- 用户在 WSL bash 里执行 PowerShell `$env:...` 语法会产生误导性报错。给验证命令时必须标明“Windows PowerShell”还是“WSL bash”。
- Windows Terminal 可能有 stable、preview、unpackaged 多个 settings 路径；installer 输出 `wrapped N profile(s)` 不等于用户打开的 profile 一定来自被改写的 settings。
- 网络网关/API 拦截方向可以拿到语义消息、token、权限或审计数据，但不能替代 PTY/ConPTY 字节流。它还可能引入账号、安全、代理和兼容性风险，不能作为终端原样显示的主方案。

### 项目关键约束和坑

- 产品主路径：Windows native daemon 拥有 HID；Windows Terminal profile 或类似 shell 层捕获 ConPTY；WSL 主要是开发和被捕获 shell 的运行环境。
- `install-windows` 默认必须保持 `wsl install: skipped`。只有用户显式 `--wsl` / `--wsl-distro` 才允许改 WSL home。
- `Terminal` session 的可信内容来源只能是 `terminal.stream` 的 PTY/ConPTY bytes，不是 transcript。
- 板端 SID 由 board 分配；daemon 注册后必须通过 `REQUEST_SESSION -> SESSION_RESPONSE` 绑定，host 不能发明最终 SID。
- 如果 board 空且 daemon log 没有 `[register]`，优先查捕获入口，不要查 board UI。
- 如果有 `[register]` 但没 `[board] request session`，查 daemon 注册分支。
- 如果有 request 但没 response，查 HID/板端。
- 如果有 response 但屏幕空，才查板端 UI/focus/VT100 replay。

### 下次一次性达到目标的提示词

```text
接手 vibe-bridge，按闭环控制方式工作：目标→状态→误差→控制动作→反馈→修正→验证→沉淀。

先读 AGENTS.md、HANDOFF.md、C_context/MEMORY.md、/home/rv_nano/Sipeed/C_context/KNOWN_FAILURES.md，并运行 /home/rv_nano/Sipeed/T_tools/agent_preflight.py --project vibe-bridge。先汇报当前状态，不要改代码。

目标：Windows 产品环境只启用一次，不要求用户每个目录/agent/入口单独安装；普通 codex/claude 必须不被破坏；板端要显示真实终端/Codex TUI，不能用 transcript 摘要、历史对话或 turn 文本冒充。

当前重点：验证 Windows Terminal profile 是否真正命中 terminal-shim。请用 daemon log 的 [terminal-shim] start、[register]、[board] request session、[board] session response 作为分段证据。如果缺 [register]，优先查 profile/settings/shim，不要查 Codex transcript 或板端 UI。

策略边界：纯被动扫描不能 1:1 还原终端；网络网关可作为语义/权限/审计辅助，但不能替代 PTY/ConPTY VT100 捕获。不要默认安装 WSL wrapper，不要替换 ~/.local/bin/codex 或 ~/.local/bin/claude。

需要我配合 Windows/真板时，给完整 PowerShell 或 WSL 命令、预期输出和失败分支。
```

### 已沉淀为 skill

- 项目内已有 `C_context/skills/vibe-bridge-control-loop/SKILL.md`。
- 下次做 vibe-bridge 的 Windows daemon、WSL、HID、terminal mirror、session/SID、Codex/Claude wrapper 恢复时，应优先使用该 skill。
- 本次已把最新 terminal profile capture 排查分支补入 skill：先看 `[terminal-shim] start`，再看 `[register]`，再看 board request/response。

## 2026-05-23 wrapper 事故与最终策略记忆

### 用户偏好

- 用户要的是**最终可观察结果**：终端显示什么，板端进度/对话框尽量一模一样显示；session 摘要、turn 文本、美化卡片都不能冒充“终端还原”。
- 用户强烈要求不干扰正常工作流。普通 `codex` / `claude` 必须仍能像原来一样启动；任何接管 CLI、改 PATH、改 wrapper 的方案都必须是显式 opt-in，并且有一键恢复路径。
- 用户会快速配合 Windows/WSL/真板实测，但不希望自己承担探索性排错。我的输出要给明确目标、状态、误差、控制动作、反馈、修正、验证、沉淀。
- 用户允许我反驳，但反驳必须基于事实约束。这里应直接指出：纯被动扫描不能保证 1:1 终端镜像。
- 用户需要 3 天后 30 秒接上，因此长任务结束必须把最新状态写到 `HANDOFF.md` 顶部，而不是让下一轮从历史段落里猜。

### 从错误里学到的最佳实践

- 不要默认替换 `~/.local/bin/codex` / `~/.local/bin/claude`。本轮 WSL wrapper 指向 `/mnt/c/Users/Administrator/AppData/Local/vibe-bridge/bin/vb-daemon.exe`，但当前 WSL 环境没有可用的该路径，直接导致 CLI 打不开。
- `real-bin` 备份也要校验内容。`~/.local/share/vibe-bridge/real-bin/claude` 曾指向已经被 wrapper 覆盖的 `2.1.148`，看起来像真实入口，实际仍是坏 wrapper。
- 恢复 CLI 时先用 `ls -la`、`readlink -f`、`file`、`--version` 验证，不要只改 symlink 后宣称恢复。
- Windows 产品路径和 WSL 开发路径必须分开。Windows native daemon 可以常驻并拥有 HID；WSL home 里的 CLI 不能被默认改写。
- `vibe-keyboard` 经验不能被过度外推。它验证的是 hook/session/permission/state machine；没有验证任意已打开终端的 VT100 捕获。
- passive transcript scan 只能做发现/摘要/fallback。它拿不到正在运行 TUI 的完整屏幕状态，不能保证“本地终端显示什么，板端显示什么”。
- 要 1:1 终端镜像，必须控制或接入 PTY/ConPTY 字节流。最佳方向是显式 `capture-shell` / `vibe-terminal`，捕获整个 shell，而不是替换 agent 二进制。
- 文档要覆盖旧方向。旧 HANDOFF 中“shim 自动捕获”一度是目标，但本轮被事故证伪；最新结论必须写在顶部，避免下一轮继续沿旧路线。

### 项目关键约束和坑

- 当前默认安装行为应是：`install-windows` 安装 Windows daemon / startup / Windows shim，但 **WSL wrapper 默认 skipped**。只有用户显式传 `--wsl` / `--wsl-distro` 才允许改 WSL home。
- 板端 SID 是 board-assigned 权威。session 刷到 256 的风险来自 host 反复注册/删除造成重复 `REQUEST_SESSION`；passive prune 不能删除 hook/launch 注册 agent，也不能删除尚未拿到 board sid 的 pending passive agent。
- `rv_nano` 已恢复到：
  - `~/.local/bin/codex -> ~/.nvm/versions/node/v22.22.2/bin/codex`
  - `~/.local/bin/claude -> ~/.local/share/claude/versions/2.1.147`
  - 坏 wrapper 保留为 `*.vibe-bridge-wrapper-broken`。
- `slam` 后续由用户恢复：`codex` 和 `claude` 均已恢复。下一轮如果要装 hook，必须先确认 slam 视角能访问 hook 脚本路径。
- Windows daemon 最新日志已能看到 HID `vid_359f&pid_2120&mi_04`，板端当前只有一个 session，未继续刷 SID；但 M4.3 permission allow/deny 真闭环还未完成。
- Codex 没有当前已知的 Claude Code `PreToolUse` 等价 hook。Codex 新开普通 session 不被识别是当前架构限制，不是单纯 bug。

### 下次一次性达到目标的提示词

```text
接手 vibe-bridge。先读 AGENTS.md、HANDOFF.md、C_context/MEMORY.md 和 C_context/KNOWN_FAILURES.md，运行 Sipeed/T_tools/agent_preflight.py --project vibe-bridge。

目标：不干扰普通 codex/claude 启动；默认 install-windows 不修改任何 WSL ~/.local/bin；板端最终要显示真实终端画面，不要用 transcript 摘要冒充。

请按“目标→状态→误差→控制动作→反馈→修正→验证→沉淀”闭环推进。先确认 rv_nano/slam 的 codex/claude 入口、Windows daemon 日志、HID 状态和板端 session 数。

策略约束：纯被动扫描不能保证 1:1 终端镜像；如果要还原终端，优先设计显式 capture-shell/vibe-terminal，用 ConPTY 捕获整个 WSL shell。Claude hook 只负责权限审批，终端镜像由 PTY/ConPTY 字节流负责。不要默认 wrapper 接管 CLI。

先汇报状态和方案利弊，不要直接改代码。需要我在 Windows/板端实测时，给完整命令、预期输出和失败分支。
```

## M4.3 permission 反向链路记忆，2026-05-21

## 用户偏好

- 用户要结果导向，但不是“看起来完成”。当我说 M4.2 完成时，用户会追问“是不是应该做收尾、完成通信链路最后一步”；这说明需要区分阶段性可见结果和真正产品闭环。
- 用户希望我可以直接反驳，但反驳必须带证据、风险、可执行替代方案和验证路径。
- 用户不想反复承担探索性验证。我的职责是先把 WSL 本地测试、交叉编译、rootfs 同步、SHA 校验、失败复盘做完，再让用户跑最后无法替代的 Windows/真板 smoke。
- 用户需要明确命令、端口、预期结果和失败分支。比如 daemon 曾跑在 `18765`，但 hook 默认 `8765`；回复必须直接指出端口不一致会导致 hook 连不上。
- 用户会在最终冲刺阶段快速配合硬件操作，但需要我说明“现在该编新固件/pack/rootfs，还是只重启 daemon”这类交付边界。
- 用户要求交接文件能 3 天后 30 秒接上；长任务结束时必须写结构化 HANDOFF 和 memory，不依赖对话记忆。

## 从错误里学到的最佳实践

- 不要把 M4.2 的“pending permission 已显示”当成项目完成。显示链路是 host -> board；审批完成还需要 board -> daemon -> hook -> Claude Code 的反向链路。
- 协议层已有 enum/helper 不代表链路已通。`PERMISSION_RES(0x12)` 已在 `vb-protocol` 存在，但 daemon 没消费、hook 没 poll 时，它仍然不是产品能力。
- Hook 返回 `{"continue":true}` 只说明 Claude 没被阻塞，不说明上报内容、daemon 处理或板端显示正确。必须看 adapter send JSON、daemon response、HID payload、板端最终显示。
- 当 hook 输出格式会影响 Claude Code 行为时，要查官方 docs，不凭记忆猜字段。当前使用 `hookSpecificOutput.permissionDecision` 返回 allow/deny/ask。
- WSL 验证和 Windows 产品验证必须分开写。`cargo check --target x86_64-pc-windows-gnu` 只能证明可编译，不能证明 Windows HID/Claude hook/真板行为。
- 本机编译板端 sample 前要警惕残留交叉编译 `.o`。`kitty_graphics.o: Relocations in generic ELF (EM: 243)` 是 RISC-V object 被 x86 linker 误用，正确修正是 `make clean` 后重建。
- 本机验证不要留下无关 `.d` 副产物；可用 `make DEPFLAGS=` 避免新增未跟踪依赖文件。
- 板端运行代码变更不能只改 source。交付到 pack 前一步必须交叉编译、strip，并同步 source binary、overlay、buildroot target、install rootfs，然后用 sha256 确认一致。
- daemon 卡住退不掉时，先让用户 `Ctrl+C`；不行再按端口找监听进程并 `Stop-Process -Force`。端口和进程状态要给精确 PowerShell 命令。

## 项目关键约束和坑

- 当前主线是 Rust `vb-daemon` + Node Claude Code hook + AIKB 板端 C 程序，不要回到旧 Python `vibe_bridge.main windows daemon/doctor` 作为产品主路径。
- Windows native daemon 是产品 HID owner；WSL 只负责代码、测试、板端编译和辅助验证。
- Claude hook 默认连接 `127.0.0.1:8765`。如果 daemon 用 `18765`，必须设置 `VIBE_BRIDGE_PORT=18765`；最终收尾建议统一 `8765`。
- Board-assigned SID 仍是权威；host 不能自己发明 sid。`agent.register` / `SessionStart` 触发 `REQUEST_SESSION`，board 返回 `SESSION_RESPONSE` 后 host 才绑定。
- `aikb_hid_input` 是板端 HID/FIFO 桥；`aikb_lcd_ui` 是板端 picker/UI owner。permission 决策路径是 `lcd_ui --ui-ctrl-out` -> `hid_input --ui-ctrl-in` -> HID `PERMISSION_RES` -> daemon -> hook poll。
- Picker 普通 session 模式和 permission 模式不能混淆。普通 `CONFIRM` 是 focus；pending permission 下 `CONFIRM` 是 allow，`REJECT` 是 deny。
- `PreToolUse` 当前策略：daemon 不在线则放行，避免卡死 Claude Code；daemon 在线但板端超时不决策则返回 `ask`，交回 Claude Code 原生确认。
- 已同步的 M4.3 板端二进制 SHA：
  - `aikb_hid_input`: `63ffeee68fa323bcb3afa6bd48196b550df934c8bef344a9049f7ab2d95cddfc`
  - `aikb_lcd_ui`: `c178b9922cdefb471a263dd5202d42f0466fda70673986a4c04d9a8362cce3e9`
- 未运行 `pack_rootfs` / `pack_burn_image` 就不能说最终镜像已包含改动；只能说源码、overlay、target rootfs、install rootfs 已准备到 pack 前一步。
- 真正剩余风险是 Windows native + 真板 allow/deny smoke、Claude Code 真实 hook schema、Windows HID 1167 长时间稳定性、daemon/board session 表重同步策略。

## 用户偏好

- 默认中文，直接、具体，不要客套。
- 不要只修一个局部小 bug 后停止；这个项目要按完整产品链路推进。
- 不要让用户反复帮忙做探索性验证；我应先做足本地验证和策略复盘，再给用户跑最终 Windows 实测。
- 用户接受 Rust/Tauri 重构，但要求保留现有 bridge 功能、HID/board 协议和 board-assigned `session_id`。
- 用户不想要“识别终端窗口”本身；终端只是容器，真正目标是识别和接管 agent sessions。
- 用户会用 Windows 真实环境验证；回复里要给明确命令和预期结果。
- 如果策略错了，要承认并换控制变量，不要继续沿着失败方向微调。

## 从错误里学到的最佳实践

- Windows 上“hook 所有终端”不能理解成单一 stdout 捕获问题；更稳的架构是 active registration / launcher / shell integration + transcript/live source + focus target state machine。
- Passive transcript scanning 会包含历史会话，不能直接等价 active sessions。
- Windows process scanning 对 WSL agent 不可靠，不能作为主 evidence。
- Win32 顶层窗口标题看不到 Windows Terminal 非激活 tab/pane，必须用 UI Automation 或主动注册。
- Tab title/cwd/name 匹配是启发式，只能作为 fallback；普通 shell title 必须过滤，例如 `npm`、PowerShell、cmd、`user@host: ~`。
- `Claude Code` / `Codex` 这种通用 title 不带 cwd/name，需要按 agent kind 绑定最新 transcript candidate，但这仍是推断，不是最终架构。
- 单测应直接固化用户真实 Windows 输出样本，避免继续凭想象调整策略。
- 每次用户给出新反馈时，先判断误差来源：source discovery、active evidence、title filtering、status inference、daemon merge，不能盲目改。

## 项目关键约束和坑

- board HID 协议不能破坏；board-assigned `session_id` 是权威。
- 正式 Windows 产品路径是 native Windows daemon 独占 HID；WSL hidraw 不是正式底座。
- `vb-host agents` 默认应输出 active/open sessions；`vb-host agents --all` 才输出历史 transcript candidates。
- `vb-daemon snapshot` 目前只是 passive discovery + registration skeleton；`registered=0` 正常，尚未接入真实 HID/board sid。
- `terminal_hwnd=unbound` 当前正常，说明还没完成 focus target/binding，不是 discovery 失败。
- `status=idle/running` 当前来自 transcript 和 open evidence，不能保证等价真实前台 TUI 状态。
- Windows Terminal tab title evidence 已能跑通当前 4 sessions，但它仍是启发式；下一阶段必须做主动注册/launcher。
- 不要重走已经失败的路线：只扫进程、只扫顶层窗口、只用 24h transcript、只按 cwd basename 匹配标题。
- 项目内已有 Python bridge/daemon/wrapper 路径，不要在 Rust/Tauri 迁移中遗忘或破坏旧功能。
- 变更前后必须更新 `HANDOFF.md`，让 3 天后能 30 秒接上。

## M4.2 Windows hook/HID 闭环记忆，2026-05-21

## 用户偏好

- 用户要结果导向，但不是盲目快；当问题反复时，需要我停止猜测，拿证据定责。
- 用户不接受“返回 ok 就算成功”。板端最终显示、daemon 日志、adapter 实际 send JSON 都要能闭环解释。
- 用户希望我可以反驳，但必须给出可执行的验证路径和利弊，不要让用户重复跑无区分度的命令。
- Windows 端测试指令要完整、可复制、带预期结果；复杂诊断应给一整套流程而不是碎片命令。
- 当我改错或方向偏了，要明确承认误差来源并更换控制变量，不要继续沿旧假设微调。

## 从错误里学到的最佳实践

- `{"continue":true}` 只表示 hook 不阻塞 Claude，不表示 hook 上报内容正确。必须用 debug 打印 adapter -> daemon 的 JSON。
- 板端显示 `unknown {}` 时，优先分段验证：PowerShell payload -> adapter stdin parse -> adapter send JSON -> daemon payload encode -> HID flush -> board ctrl-out -> LCD display。
- PowerShell 管道到 Node 可能出现 BOM/UTF-16LE/NUL/不可见字符问题；Node adapter 读 stdin 应用 `fs.readFileSync(0)` 读 Buffer，再做编码清理和 parse error debug。
- Windows PowerShell 的 `Set-Content -Encoding UTF8` 可能写入 BOM，Node 会在 shebang 前报 `SyntaxError: Invalid or unexpected token`。写 JS 文件要用无 BOM UTF-8。
- daemon 不应在 TCP response 前同步等待 HID flush，否则 HID `WriteFile` 卡住会让客户端 `$reader.ReadLine()` 卡死。
- HID `WriteFile failed with Win32 error 1167` 应按设备句柄失效/重枚举处理，daemon 不能直接退出；需要 timeout、取消 pending I/O、reopen/backoff。
- hook adapter 每个事件前重复 `agent.register` 是正常行为；daemon 必须幂等处理，不能重放历史 pending permission 污染板端最新显示。

## 项目关键约束和坑

- Windows native daemon 是 HID owner；adapter 只走 TCP IPC，不直接打开 HID。
- board-assigned SID 仍是权威；`permission.request` 不创建 SID，只有 `agent.register` / `SessionStart` 触发 `REQUEST_SESSION`。
- 板端 SESSION 页面显示的是 daemon 发来的最新 session state/turn/permission；旧 SID/旧 pending 会造成误判，必要时先重启 daemon 或设计 reset/sync。
- M4.2 当前已手工验证：`SessionStart -> UserPromptSubmit -> PreToolUse` 能在板端显示 SID、`user: hello board`、`Bash {"command":"cargo test --workspace"}`。
- `PreToolUse` 目前仍是 non-blocking：返回 `continue:true`，permission 只展示，不会等待板端 allow/deny。
- 真实 Claude Code hook schema 还未最终确认；当前 adapter 兼容 `tool_name/toolName/tool/name` 和 `tool_input/toolInput/input/args`。
