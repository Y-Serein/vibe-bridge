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
    Unknown,
}

impl AgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Unknown => "unknown",
        }
    }

    pub fn from_label(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" => Self::Claude,
            "codex" | "codex-cli" | "codex-tui" => Self::Codex,
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
