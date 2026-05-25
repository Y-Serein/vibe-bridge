//! Core host-side models shared by the Rust vibe-bridge backend.
//!
//! The board protocol remains owned by the existing HID packet contract. This
//! crate defines host discovery/session metadata and the Rust-side binding
//! model used before an agent is bound to a board-assigned session id.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalKind {
    WindowsTerminal,
    ConsoleHost,
    PowerShell,
    Cmd,
    Wsl,
    TerminalLike,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentKind {
    Claude,
    Codex,
    Terminal,
    Unknown,
}

impl AgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Terminal => "terminal",
            Self::Unknown => "unknown",
        }
    }

    pub fn from_label(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" => Self::Claude,
            "codex" | "codex-cli" | "codex-tui" => Self::Codex,
            "terminal" | "shell" | "vibe-terminal" | "capture-shell" => Self::Terminal,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Running,
    WaitingInput,
    Idle,
    Done,
    Error,
    Unknown,
}

impl AgentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::WaitingInput => "waiting-input",
            Self::Idle => "idle",
            Self::Done => "done",
            Self::Error => "error",
            Self::Unknown => "unknown",
        }
    }
}

impl TerminalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WindowsTerminal => "windows-terminal",
            Self::ConsoleHost => "console-host",
            Self::PowerShell => "powershell",
            Self::Cmd => "cmd",
            Self::Wsl => "wsl",
            Self::TerminalLike => "terminal-like",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalWindow {
    pub hwnd: usize,
    pub pid: u32,
    pub title: String,
    pub process_path: String,
    pub kind: TerminalKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSession {
    pub agent_id: String,
    pub kind: AgentKind,
    pub name: String,
    pub cwd: String,
    pub transcript_path: String,
    pub status: AgentStatus,
    pub terminal_hwnd: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentActivityKind {
    Seen,
    UserInput,
    AssistantOutput,
    ToolActivity,
    WaitingInput,
    Completed,
    Error,
}

impl AgentActivityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Seen => "seen",
            Self::UserInput => "user-input",
            Self::AssistantOutput => "assistant-output",
            Self::ToolActivity => "tool-activity",
            Self::WaitingInput => "waiting-input",
            Self::Completed => "completed",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentActivity {
    pub agent_id: String,
    pub kind: AgentKind,
    pub activity: AgentActivityKind,
    pub status: AgentStatus,
    pub transcript_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BridgeSessionBinding {
    pub board_sid: u16,
    pub terminal_hwnd: Option<usize>,
}

impl TerminalWindow {
    pub fn process_basename(&self) -> &str {
        self.process_path
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(self.process_path.as_str())
    }

    pub fn is_terminal_candidate(&self) -> bool {
        self.kind != TerminalKind::Unknown
    }
}

pub fn classify_terminal_process(process_path: &str, title: &str) -> TerminalKind {
    let exe = process_path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(process_path)
        .to_ascii_lowercase();
    let title = title.to_ascii_lowercase();
    let trusted_title_only_host =
        exe.is_empty() || matches!(exe.as_str(), "applicationframehost.exe");

    match exe.as_str() {
        "windowsterminal.exe" | "wt.exe" => TerminalKind::WindowsTerminal,
        "openconsole.exe" | "conhost.exe" => TerminalKind::ConsoleHost,
        "powershell.exe" | "pwsh.exe" => TerminalKind::PowerShell,
        "cmd.exe" => TerminalKind::Cmd,
        "wsl.exe" | "ubuntu.exe" | "ubuntu2204.exe" | "ubuntu-22.04.exe" => TerminalKind::Wsl,
        "wezterm-gui.exe" | "alacritty.exe" | "mintty.exe" | "tabby.exe" | "hyper.exe" => {
            TerminalKind::TerminalLike
        }
        _ if trusted_title_only_host && title.contains("windows powershell") => {
            TerminalKind::PowerShell
        }
        _ if trusted_title_only_host && title.contains("command prompt") => TerminalKind::Cmd,
        _ if trusted_title_only_host && (title.contains("ubuntu") || title.contains("wsl")) => {
            TerminalKind::Wsl
        }
        _ if trusted_title_only_host && title.contains("windows terminal") => {
            TerminalKind::WindowsTerminal
        }
        _ => TerminalKind::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_windows_terminals() {
        assert_eq!(
            classify_terminal_process(r"C:\Program Files\WindowsApps\WindowsTerminal.exe", ""),
            TerminalKind::WindowsTerminal
        );
        assert_eq!(
            classify_terminal_process(
                r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
                ""
            ),
            TerminalKind::PowerShell
        );
        assert_eq!(
            classify_terminal_process(r"C:\Windows\System32\cmd.exe", ""),
            TerminalKind::Cmd
        );
        assert_eq!(
            classify_terminal_process(r"C:\Windows\System32\wsl.exe", ""),
            TerminalKind::Wsl
        );
    }

    #[test]
    fn title_can_identify_wsl_windows() {
        assert_eq!(
            classify_terminal_process(
                "ApplicationFrameHost.exe",
                "Ubuntu-22.04 - Windows Terminal"
            ),
            TerminalKind::Wsl
        );
    }

    #[test]
    fn document_titles_do_not_create_terminal_false_positive() {
        assert_eq!(
            classify_terminal_process("wps.exe", "hid_vendor_terminal_spec.pdf - WPS Office"),
            TerminalKind::Unknown
        );
    }
}

// ============================================================================
// Board / Agent 跨端通信类型 (M1 起新增)
// 这些类型被 vb-protocol / vb-transport / vb-agent / vb-daemon 共享。
// 保持零依赖 (no serde/no_std-compatible style); IPC 序列化在 vb-daemon 那
// 一层用 serde derive 包一层 wrapper 类型。
// ============================================================================

/// 板端分配的 session id。0 保留为广播 (与 aikb_hid_input.c SESSION_BROADCAST 对齐)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BoardSid(pub u16);

impl BoardSid {
    pub const BROADCAST: Self = BoardSid(0);

    pub const fn new(raw: u16) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u16 {
        self.0
    }

    pub const fn is_broadcast(self) -> bool {
        self.0 == 0
    }
}

/// 一个 session 的 token / 费用快照, 板端按 sid 显示。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    /// 以分计 (USD cents)。板端显示按需缩放为美元。
    pub cost_cents: u64,
}

/// 一轮对话发言。短消息直接板端显示, 长消息分包发 TURN_APPEND。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationTurn {
    pub sid: BoardSid,
    pub role: TurnRole,
    pub text: String,
    pub ts_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnRole {
    User,
    Assistant,
    Tool,
    System,
}

impl TurnRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
            Self::System => "system",
        }
    }
}

/// agent 发出的待审批请求 (Claude Code PreToolUse / Codex tool gate 等)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequest {
    pub req_id: u64,
    pub sid: BoardSid,
    pub tool: String,
    pub args_summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Deny,
    Always,
}

impl PermissionDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Always => "always",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub sid: BoardSid,
    pub level: NotificationLevel,
    pub text: String,
    pub ts_ms: u64,
}

#[cfg(test)]
mod board_types_tests {
    use super::*;

    #[test]
    fn board_sid_broadcast_is_zero() {
        assert!(BoardSid::BROADCAST.is_broadcast());
        assert!(!BoardSid::new(1).is_broadcast());
        assert_eq!(BoardSid::new(42).raw(), 42);
    }

    #[test]
    fn turn_role_labels() {
        assert_eq!(TurnRole::User.as_str(), "user");
        assert_eq!(TurnRole::Assistant.as_str(), "assistant");
    }

    #[test]
    fn permission_decision_labels() {
        assert_eq!(PermissionDecision::Allow.as_str(), "allow");
        assert_eq!(PermissionDecision::Deny.as_str(), "deny");
        assert_eq!(PermissionDecision::Always.as_str(), "always");
    }
}
