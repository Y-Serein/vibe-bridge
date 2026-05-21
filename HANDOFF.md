# vibe-bridge HANDOFF

## 当前在做什么

当前阶段目标是把 `vibe-bridge` 从早期 Python wrapper/daemon 原型推进到 Windows 可用的 Rust/Tauri host control plane，同时保留既有 HID/board 协议和 board-assigned `session_id`。

截至 2026-05-21，Windows agent discovery 这一阶段已跑通用户实测闭环：

- `cargo run -p vb-host -- tabs` 能通过 Windows Terminal UI Automation 看到 tab titles，包括 `rv_nano`、`Typora`、`✳ Claude Code`。
- `cargo run -p vb-host -- agents` 能输出 4 个当前打开的 agent sessions：
  - Claude：`vibe-bridge`
  - Codex：两个 `rv_nano`
  - Codex：`Typora`
- `cargo run -p vb-daemon -- snapshot` 能输出同一批 4 个 sessions，`registered=0`，说明当前仍是 passive discovery + daemon snapshot skeleton，还没进入 board sid/HID binding。
- 当前策略是 evidence fusion：
  - Codex/Claude transcript candidate 是 session 事实来源。
  - WSL home discovery 找到 Windows 下的 WSL transcript roots。
  - Windows Terminal tab title/UIA 证明 session 仍打开。
  - process scanning 只能作为辅助，不能作为 Windows->WSL 主信号。

当前可以认为“发现现有 Windows/WSL Codex/Claude agent session”阶段完成；下一阶段应转向 Rust daemon 拥有 HID、board sid binding、focus target 和主动注册。

## 已经试过的方案和结果（含失败的）

- 失败：只靠 Python Windows scanner / ToolHelp32 扫进程。
  - 结果：`agent-scanner` 能看到进程总数，但检测不到 WSL 中已有 Codex/Claude。
  - 结论：Windows 进程枚举不能作为 WSL agent 主链路。

- 失败：只枚举 Win32 顶层窗口。
  - 结果：只能看到 Windows Terminal 顶层窗口，非激活 tab/pane 中的 agent 会漏掉。
  - 修正：新增 `vb-host tabs`，用 UI Automation 枚举 Windows Terminal `TabItem` 标题；失败时才回退顶层窗口标题。

- 失败：只按 transcript 最近 24h 过滤。
  - 结果：历史 Codex/Claude JSONL 被当作 active session，用户机器一度看到 9 个 sessions。
  - 修正：默认 `agents` / `snapshot` 只输出 active/open sessions；`agents --all` 才输出历史候选。

- 失败：只按 process evidence 过滤 active sessions。
  - 结果：用户 Windows 上 `vb-host processes` 返回 0，但实际终端里有打开的 Codex。
  - 结论：process evidence 在 Windows->WSL 路径不可靠；tab title evidence 必须参与 active 判定。

- 失败：用 cwd basename 直接匹配 tab title。
  - 结果：`rv_nano@DESKTOP-J9FG6TS: ~` 这种普通 WSL shell 被误判成 agent。
  - 修正：过滤普通 shell titles，包括 `npm`、PowerShell、cmd、`user@host: ~` / `name@host: /path`。

- 失败：只按 session name/cwd 匹配 tab title。
  - 结果：`✳ Claude Code` 这种通用标题不含 cwd/name，新增 Claude 被漏掉，`agents` 只有 3 个。
  - 修正：如果 title 明确包含 `claude` / `codex` 且没命中具体 label，就按 agent kind 选择该 kind 最新 transcript candidate 作为 open session evidence。

- 已通过验证：
  - `cargo fmt --all`
  - `env CARGO_TARGET_DIR=/tmp/vibe-bridge-rust-target cargo test --workspace`
  - `env CARGO_TARGET_DIR=/tmp/vibe-bridge-tauri-target cargo check`
  - `git diff --check`
  - 用户 Windows 实测 `tabs/agents/snapshot` 均达到预期 4 sessions。

## 下一步计划（3-5条actionable)

1. 在 `vb-daemon` 中接入真正 HID owner：daemon 独占 Windows HID 设备，继续保留 board-assigned `session_id` 为权威。
2. 给每个 active agent 建立 `BridgeSessionBinding`：`agent_id/kind/cwd` -> `board_sid` -> optional `terminal_hwnd/focus target`。
3. 增加主动注册/launcher/shell integration：Windows/WSL CLI、VS Code、browser adapter 通过本地 IPC 注册 agent session，tab title 只作为 fallback evidence。
4. 把 `AgentSourcePoller` 的 activity event 转成 board 可消费的 `REQUEST_SESSION` / `STATUS_UPDATE` / `SESSION_FOCUS` / terminal replay 路由。
5. Tauri dashboard 读取 daemon snapshot，展示 agent sessions、bindings、HID 状态、board sid 和 last activity；不要让前端拥有协议或 HID 逻辑。

## 关键文件路径（相对路径，一行一个）

Cargo.toml
Cargo.lock
crates/vb-core/src/lib.rs
crates/vb-host/src/agent_discovery.rs
crates/vb-host/src/main.rs
crates/vb-host/src/lib.rs
crates/vb-daemon/src/lib.rs
crates/vb-daemon/src/main.rs
desktop/src-tauri/Cargo.toml
desktop/src-tauri/src/lib.rs
desktop/src/main.tsx
src/vibe_bridge/main.py
src/vibe_bridge/windows_host.py
src/vibe_bridge/windows_runner.py
src/vibe_bridge/transport_win_hid.py
scripts/windows_status.ps1
scripts/windows_input_watch.ps1
scripts/windows_session_smoke.ps1
README.md
AGENTS.md
C_context/MEMORY.md

## 还没搞清楚的问题

- Windows Terminal UI Automation 是否能稳定区分 tab 和 pane；如果 pane 不暴露为 `TabItem`，仍必须依赖主动注册/launcher。
- `✳ Claude Code` 这种通用标题只能推断“最新 Claude transcript 仍打开”，无法保证一定绑定到正确 tab；下一阶段要用主动注册消除这个不确定性。
- `status=idle/running` 当前主要来自 transcript 事件和 open evidence，不能等同于真实 TUI 前台状态。
- `vb-daemon snapshot` 现在还没有接 HID/board sid；`registered=0` 是预期状态，不代表板端 session 已建立。
- 还没验证 Rust daemon 在真实 Windows HID 上长期运行、热插拔、board SESSION UI、旋钮/按键回路。
