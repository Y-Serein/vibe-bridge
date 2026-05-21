//! Windows host control plane for vibe-bridge.
//!
//! The binary CLI and future Tauri commands both call this library. Keep
//! platform-specific Win32 details behind this boundary so the UI/control plane
//! does not duplicate discovery and focus logic.

use vb_core::{AgentSession, TerminalWindow};

mod agent_discovery;
pub use agent_discovery::{
    active_agent_processes, agent_source_roots, ActiveAgentProcess, AgentPollSnapshot,
    AgentSourcePoller,
};

#[cfg(windows)]
mod windows_terminal;

#[cfg(not(windows))]
mod windows_terminal {
    use vb_core::TerminalWindow;

    pub fn discover_terminal_windows() -> Result<Vec<TerminalWindow>, String> {
        Err("Windows terminal discovery only runs on native Windows".to_string())
    }

    pub fn focus_window(_hwnd: usize) -> Result<(), String> {
        Err("Windows window focus only runs on native Windows".to_string())
    }

    pub fn discover_terminal_titles() -> Result<Vec<String>, String> {
        Err("Windows terminal title discovery only runs on native Windows".to_string())
    }
}

pub fn discover_terminal_windows() -> Result<Vec<TerminalWindow>, String> {
    windows_terminal::discover_terminal_windows()
}

pub fn focus_window(hwnd: usize) -> Result<(), String> {
    windows_terminal::focus_window(hwnd)
}

pub fn discover_terminal_titles() -> Result<Vec<String>, String> {
    windows_terminal::discover_terminal_titles()
}

pub fn discover_agent_sessions() -> Result<Vec<AgentSession>, String> {
    agent_discovery::discover_agent_sessions()
}

pub fn discover_agent_session_candidates() -> Result<Vec<AgentSession>, String> {
    agent_discovery::discover_agent_session_candidates()
}
