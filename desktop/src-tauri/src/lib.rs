use vb_core::{AgentSession, TerminalWindow};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentSessionDto {
    agent_id: String,
    kind: String,
    name: String,
    cwd: String,
    transcript_path: String,
    status: String,
    terminal_hwnd: Option<usize>,
}

impl From<AgentSession> for AgentSessionDto {
    fn from(session: AgentSession) -> Self {
        Self {
            agent_id: session.agent_id,
            kind: session.kind.as_str().to_string(),
            name: session.name,
            cwd: session.cwd,
            transcript_path: session.transcript_path,
            status: session.status.as_str().to_string(),
            terminal_hwnd: session.terminal_hwnd,
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalWindowDto {
    hwnd: usize,
    pid: u32,
    title: String,
    process_path: String,
    kind: String,
}

impl From<TerminalWindow> for TerminalWindowDto {
    fn from(window: TerminalWindow) -> Self {
        Self {
            hwnd: window.hwnd,
            pid: window.pid,
            title: window.title,
            process_path: window.process_path,
            kind: window.kind.as_str().to_string(),
        }
    }
}

#[tauri::command]
fn discover_terminal_windows() -> Result<Vec<TerminalWindowDto>, String> {
    vb_host::discover_terminal_windows()
        .map(|windows| windows.into_iter().map(TerminalWindowDto::from).collect())
}

#[tauri::command]
fn discover_agent_sessions() -> Result<Vec<AgentSessionDto>, String> {
    vb_host::discover_agent_sessions()
        .map(|sessions| sessions.into_iter().map(AgentSessionDto::from).collect())
}

#[tauri::command]
fn focus_terminal_window(hwnd: usize) -> Result<(), String> {
    vb_host::focus_window(hwnd)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            discover_agent_sessions,
            discover_terminal_windows,
            focus_terminal_window,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Vibe Bridge Desktop");
}
