use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};

use serde::Deserialize;
use serde_json::json;
use vb_core::{AgentActivity, AgentActivityKind, AgentKind, AgentSession, AgentStatus};
use vb_host::AgentSourcePoller;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredAgent {
    pub agent_id: String,
    pub kind: AgentKind,
    pub name: String,
    pub cwd: String,
    pub focus_target: Option<String>,
    pub terminal_hwnd: Option<usize>,
    pub status: AgentStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonSnapshot {
    pub sessions: Vec<AgentSession>,
    pub activities: Vec<AgentActivity>,
    pub registered_agents: usize,
}

pub struct BridgeDaemon {
    poller: AgentSourcePoller,
    registered: HashMap<(AgentKind, String), RegisteredAgent>,
    activities: Vec<AgentActivity>,
}

impl BridgeDaemon {
    pub fn new() -> Self {
        Self {
            poller: AgentSourcePoller::new(),
            registered: HashMap::new(),
            activities: Vec::new(),
        }
    }

    pub fn poll_once(&mut self) -> Result<DaemonSnapshot, String> {
        let polled = self.poller.poll_once()?;
        self.activities.extend(polled.activities);

        let mut sessions = polled.sessions;
        for registered in self.registered.values() {
            let key_exists = sessions.iter().any(|session| {
                session.kind == registered.kind && session.agent_id == registered.agent_id
            });
            if key_exists {
                continue;
            }
            sessions.push(registered.to_session());
        }
        sessions.sort_by(|a, b| b.agent_id.cmp(&a.agent_id));

        Ok(DaemonSnapshot {
            sessions,
            activities: self.activities.clone(),
            registered_agents: self.registered.len(),
        })
    }

    pub fn handle_json_line(&mut self, line: &str) -> Result<String, String> {
        let request: RegistrationEnvelope =
            serde_json::from_str(line).map_err(|err| format!("invalid json: {err}"))?;
        match request.message_type.as_str() {
            "agent.register" => {
                let registration = request
                    .agent
                    .ok_or_else(|| "agent.register requires agent payload".to_string())?;
                let agent = registration.into_registered_agent()?;
                let key = (agent.kind, agent.agent_id.clone());
                self.activities.push(AgentActivity {
                    agent_id: agent.agent_id.clone(),
                    kind: agent.kind,
                    activity: AgentActivityKind::Seen,
                    status: agent.status,
                    transcript_path: String::new(),
                });
                self.registered.insert(key, agent);
                Ok(json!({"ok": true, "type": "agent.registered"}).to_string())
            }
            "agent.activity" => {
                let activity = request
                    .activity
                    .ok_or_else(|| "agent.activity requires activity payload".to_string())?;
                let event = activity.into_agent_activity()?;
                if let Some(agent) = self
                    .registered
                    .get_mut(&(event.kind, event.agent_id.clone()))
                {
                    agent.status = event.status;
                }
                self.activities.push(event);
                Ok(json!({"ok": true, "type": "agent.activity.accepted"}).to_string())
            }
            "daemon.snapshot" => {
                let snapshot = self.poll_once()?;
                Ok(json!({
                    "ok": true,
                    "type": "daemon.snapshot",
                    "sessions": snapshot.sessions.len(),
                    "activities": snapshot.activities.len(),
                    "registeredAgents": snapshot.registered_agents,
                })
                .to_string())
            }
            other => Err(format!("unknown message type: {other}")),
        }
    }
}

impl Default for BridgeDaemon {
    fn default() -> Self {
        Self::new()
    }
}

impl RegisteredAgent {
    fn to_session(&self) -> AgentSession {
        AgentSession {
            agent_id: self.agent_id.clone(),
            kind: self.kind,
            name: if self.name.is_empty() {
                self.agent_id.chars().take(8).collect()
            } else {
                self.name.clone()
            },
            cwd: self.cwd.clone(),
            transcript_path: String::new(),
            status: self.status,
            terminal_hwnd: self.terminal_hwnd,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistrationEnvelope {
    #[serde(rename = "type")]
    message_type: String,
    agent: Option<RegistrationAgentPayload>,
    activity: Option<ActivityPayload>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistrationAgentPayload {
    agent_id: String,
    kind: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    cwd: String,
    focus_target: Option<String>,
    terminal_hwnd: Option<usize>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivityPayload {
    agent_id: String,
    kind: String,
    activity: String,
    status: String,
    #[serde(default)]
    transcript_path: String,
}

impl RegistrationAgentPayload {
    fn into_registered_agent(self) -> Result<RegisteredAgent, String> {
        let kind = AgentKind::from_label(&self.kind);
        if kind == AgentKind::Unknown {
            return Err(format!("unknown agent kind: {}", self.kind));
        }
        if self.agent_id.trim().is_empty() {
            return Err("agent_id is required".to_string());
        }
        Ok(RegisteredAgent {
            agent_id: self.agent_id,
            kind,
            name: self.name,
            cwd: self.cwd,
            focus_target: self.focus_target,
            terminal_hwnd: self.terminal_hwnd,
            status: status_from_label(self.status.as_deref().unwrap_or("running")),
        })
    }
}

impl ActivityPayload {
    fn into_agent_activity(self) -> Result<AgentActivity, String> {
        let kind = AgentKind::from_label(&self.kind);
        if kind == AgentKind::Unknown {
            return Err(format!("unknown agent kind: {}", self.kind));
        }
        if self.agent_id.trim().is_empty() {
            return Err("agent_id is required".to_string());
        }
        Ok(AgentActivity {
            agent_id: self.agent_id,
            kind,
            activity: activity_from_label(&self.activity),
            status: status_from_label(&self.status),
            transcript_path: self.transcript_path,
        })
    }
}

pub fn run_tcp_registration_server<A: ToSocketAddrs>(addr: A) -> Result<(), String> {
    let listener =
        TcpListener::bind(addr).map_err(|err| format!("bind registration server: {err}"))?;
    let mut daemon = BridgeDaemon::new();
    for stream in listener.incoming() {
        let stream = stream.map_err(|err| format!("accept registration client: {err}"))?;
        handle_client(stream, &mut daemon)?;
    }
    Ok(())
}

fn handle_client(mut stream: TcpStream, daemon: &mut BridgeDaemon) -> Result<(), String> {
    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|err| format!("clone registration stream: {err}"))?,
    );
    let mut line = String::new();
    while reader
        .read_line(&mut line)
        .map_err(|err| format!("read registration line: {err}"))?
        > 0
    {
        let response = match daemon.handle_json_line(line.trim()) {
            Ok(response) => response,
            Err(err) => json!({"ok": false, "error": err}).to_string(),
        };
        writeln!(stream, "{response}")
            .map_err(|err| format!("write registration response: {err}"))?;
        line.clear();
    }
    Ok(())
}

fn status_from_label(value: &str) -> AgentStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "running" => AgentStatus::Running,
        "waiting-input" | "waiting_input" | "waiting" => AgentStatus::WaitingInput,
        "idle" => AgentStatus::Idle,
        "done" | "completed" | "complete" => AgentStatus::Done,
        "error" | "failed" => AgentStatus::Error,
        _ => AgentStatus::Unknown,
    }
}

fn activity_from_label(value: &str) -> AgentActivityKind {
    match value.trim().to_ascii_lowercase().as_str() {
        "seen" => AgentActivityKind::Seen,
        "user-input" | "user_input" | "user" => AgentActivityKind::UserInput,
        "assistant-output" | "assistant_output" | "assistant" => AgentActivityKind::AssistantOutput,
        "waiting-input" | "waiting_input" | "waiting" => AgentActivityKind::WaitingInput,
        "completed" | "complete" | "done" => AgentActivityKind::Completed,
        "error" | "failed" => AgentActivityKind::Error,
        _ => AgentActivityKind::ToolActivity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_agent_adds_snapshot_session() {
        let mut daemon = BridgeDaemon::new();
        let response = daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"codex-1","kind":"codex","name":"AIKB","cwd":"/work/AIKB","terminalHwnd":4660}}"#,
            )
            .unwrap();
        assert!(response.contains("agent.registered"));

        let snapshot = daemon.poll_once().unwrap();
        let session = snapshot
            .sessions
            .iter()
            .find(|session| session.agent_id == "codex-1")
            .unwrap();
        assert_eq!(session.kind, AgentKind::Codex);
        assert_eq!(session.name, "AIKB");
        assert_eq!(session.cwd, "/work/AIKB");
        assert_eq!(session.terminal_hwnd, Some(4660));
    }

    #[test]
    fn activity_updates_registered_status() {
        let mut daemon = BridgeDaemon::new();
        daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"claude-1","kind":"claude","cwd":"/work"}}"#,
            )
            .unwrap();
        daemon
            .handle_json_line(
                r#"{"type":"agent.activity","activity":{"agentId":"claude-1","kind":"claude","activity":"waiting-input","status":"waiting-input"}}"#,
            )
            .unwrap();

        let snapshot = daemon.poll_once().unwrap();
        let session = snapshot
            .sessions
            .iter()
            .find(|session| session.agent_id == "claude-1")
            .unwrap();
        assert_eq!(session.status, AgentStatus::WaitingInput);
    }
}
