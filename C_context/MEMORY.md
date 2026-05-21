# vibe-bridge Memory

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
