#[cfg(windows)]
pub mod conpty;

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use vb_core::{
    AgentActivity, AgentActivityKind, AgentKind, AgentSession, AgentStatus, BoardSid,
    ConversationTurn, PermissionDecision, PermissionRequest, TokenUsage, TurnRole,
};
use vb_host::{max_retained_turns_per_agent, AgentSourcePoller, DiscoveredTurn};
use vb_protocol::payloads::{
    decode_permission_response, encode_agent_meta, encode_permission_request, encode_token_usage,
    encode_turn_append,
};
use vb_protocol::{
    split_host_to_board, Cmd, PermissionDecisionByte, SessionStateByte, SessionStatusByte,
    TurnRoleByte,
};
use vb_transport::{HidMessage, HidTransport};

const VT100_BUFFER_BYTES: usize = 64 * 1024;
const PASSIVE_MISSING_GRACE_POLLS: u8 = 3;
const SCREEN_CLEAR: &[u8] = b"\x1b[2J\x1b[H";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredAgent {
    pub agent_id: String,
    pub kind: AgentKind,
    pub name: String,
    pub cwd: String,
    pub focus_target: Option<String>,
    pub terminal_hwnd: Option<usize>,
    pub status: AgentStatus,
    /// 板端分配的 sid; None 表示尚未通过 REQUEST_SESSION 拿到。M4 接 HID 后 daemon 主动申请。
    pub board_sid: Option<BoardSid>,
    /// 累计 token 用量 (从 transcript / hook 上报增量累加)。
    pub usage: TokenUsage,
    /// 对话 turn 历史 (按时间序)。
    pub turns: Vec<ConversationTurn>,
    /// 当前等用户决定的 permission 请求。
    pub pending_permissions: Vec<PermissionRequest>,
    /// 板端已经返回、等待 hook poll 取走的 permission 决策。
    pub resolved_permissions: HashMap<u64, PermissionDecision>,
    /// True after this agent registered through the JSON IPC path (Claude hook
    /// or vb-daemon launch). Passive transcript pruning must not remove these;
    /// hooks own their lifetime through session.abort / SessionEnd.
    pub from_hook: bool,
    /// True when this agent was registered by `vb-daemon launch`/`start`
    /// (daemon owns its ConPTY and emits Vt100Stream). False for passive
    /// agents discovered via transcript scan or registered by a hook from
    /// a user-started terminal.
    pub from_launch: bool,
    /// Recent raw VT100/ANSI bytes captured from this agent's PTY/ConPTY.
    /// This is the fidelity source: when the board focuses this sid, daemon
    /// replays clear+buffer so the LCD sees what the host terminal saw.
    terminal_buffer: VecDeque<Vec<u8>>,
    terminal_buffer_len: usize,
    /// Agent shims launched inside an already captured terminal do not own a
    /// second ConPTY. Their display bytes are mirrored from this parent
    /// terminal session to avoid two stdin readers fighting over CONIN$.
    attached_terminal: Option<(AgentKind, String)>,
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
    pending_board_sessions: VecDeque<(AgentKind, String)>,
    passive_missing_counts: HashMap<(AgentKind, String), u8>,
    board_outbox: VecDeque<HidMessage>,
    focused_board_sid: Option<BoardSid>,
}

impl BridgeDaemon {
    pub fn new() -> Self {
        Self {
            poller: AgentSourcePoller::new(),
            registered: HashMap::new(),
            activities: Vec::new(),
            pending_board_sessions: VecDeque::new(),
            passive_missing_counts: HashMap::new(),
            board_outbox: VecDeque::new(),
            focused_board_sid: None,
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
                let mut agent = registration.into_registered_agent()?;
                let key = (agent.kind, agent.agent_id.clone());
                eprintln!(
                    "[register] {}/{} name={} from_launch={} existing={}",
                    agent.kind.as_str(),
                    agent.agent_id,
                    agent.name,
                    agent.from_launch,
                    self.registered.contains_key(&key)
                );
                let was_already_bound = if let Some(existing) = self.registered.get(&key) {
                    agent.board_sid = existing.board_sid;
                    agent.usage = existing.usage;
                    agent.turns = existing.turns.clone();
                    agent.pending_permissions = existing.pending_permissions.clone();
                    agent.resolved_permissions = existing.resolved_permissions.clone();
                    agent.from_hook = true;
                    agent.terminal_buffer = existing.terminal_buffer.clone();
                    agent.terminal_buffer_len = existing.terminal_buffer_len;
                    if agent.attached_terminal.is_none() {
                        agent.attached_terminal = existing.attached_terminal.clone();
                    }
                    // Sticky launched flag: once an agent has been registered
                    // by `vb-daemon launch`/`start`, any later hook-driven
                    // re-register on the same agentId stays launched (the
                    // ConPTY is still ours).
                    agent.from_launch = agent.from_launch || existing.from_launch;
                    existing.board_sid.is_some()
                } else {
                    false
                };
                self.activities.push(AgentActivity {
                    agent_id: agent.agent_id.clone(),
                    kind: agent.kind,
                    activity: AgentActivityKind::Seen,
                    status: agent.status,
                    transcript_path: String::new(),
                });
                if was_already_bound {
                    // Hook adapters call agent.register before each event. Replaying all
                    // historical pending permissions here can pin the board UI to stale rows.
                } else if agent.board_sid.is_some() {
                    for msg in board_sync_messages(&agent) {
                        self.board_outbox.push_back(msg);
                    }
                } else if is_transport_only_terminal(&agent) {
                    eprintln!(
                        "[board] suppress terminal session for {}/{}; terminal remains stream source",
                        agent.kind.as_str(),
                        agent.agent_id
                    );
                } else {
                    self.enqueue_board_session_request(&agent);
                }
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
                    if let Some(msg) = status_message_for_agent(agent) {
                        self.board_outbox.push_back(msg);
                    }
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
            "token.update" => {
                let update = request
                    .token
                    .ok_or_else(|| "token.update requires token payload".to_string())?;
                let kind = parse_kind(&update.kind)?;
                if let Some(agent) = self.registered.get_mut(&(kind, update.agent_id.clone())) {
                    agent.usage = TokenUsage {
                        input: update.input,
                        output: update.output,
                        cost_cents: update.cost_cents,
                    };
                    if let Some(msg) = token_message_for_agent(agent) {
                        self.board_outbox.push_back(msg);
                    }
                    Ok(json!({"ok": true, "type": "token.accepted"}).to_string())
                } else {
                    Err(format!(
                        "no registered agent for token update: {}/{}",
                        kind.as_str(),
                        update.agent_id
                    ))
                }
            }
            "turn.append" => {
                // Hook fires this on UserPromptSubmit/tool-result/etc. We
                // store in agent.turns and, if this agent is currently
                // focused on the board AND is a passive one, push a fresh
                // composed VT100 view so the board terminal stays live
                // without the user having to exit-reenter the SID.
                let turn = request
                    .turn
                    .ok_or_else(|| "turn.append requires turn payload".to_string())?;
                let kind = parse_kind(&turn.kind)?;
                let role = parse_role(&turn.role)?;
                if let Some(agent) = self.registered.get_mut(&(kind, turn.agent_id.clone())) {
                    let sid = agent.board_sid.unwrap_or(BoardSid::BROADCAST);
                    let event = ConversationTurn {
                        sid,
                        role,
                        text: turn.text,
                        ts_ms: turn.ts_ms,
                    };
                    agent.turns.push(event);
                    let cap = max_retained_turns_per_agent();
                    if agent.turns.len() > cap {
                        let overflow = agent.turns.len() - cap;
                        agent.turns.drain(..overflow);
                    }
                    // Re-render live for passive sessions on focus.
                    let agent_ref = self
                        .registered
                        .get(&(kind, turn.agent_id.clone()))
                        .expect("just inserted");
                    if !agent_ref.from_launch
                        && agent_ref.board_sid.is_some()
                        && agent_ref.board_sid == self.focused_board_sid
                    {
                        if let Some(msg) = terminal_replay_message_for_agent(agent_ref) {
                            self.board_outbox.push_back(msg);
                        }
                    }
                    Ok(json!({"ok": true, "type": "turn.accepted"}).to_string())
                } else {
                    Err(format!(
                        "no registered agent for turn: {}/{}",
                        kind.as_str(),
                        turn.agent_id
                    ))
                }
            }
            "permission.request" => {
                let perm = request
                    .permission
                    .ok_or_else(|| "permission.request requires permission payload".to_string())?;
                let kind = parse_kind(&perm.kind)?;
                if let Some(agent) = self.registered.get_mut(&(kind, perm.agent_id.clone())) {
                    let sid = agent.board_sid.unwrap_or(BoardSid::BROADCAST);
                    let event = PermissionRequest {
                        req_id: perm.req_id,
                        sid,
                        tool: perm.tool,
                        args_summary: perm.args_summary,
                    };
                    agent.resolved_permissions.remove(&event.req_id);
                    agent.pending_permissions.push(event);
                    if let Some(msg) = agent
                        .pending_permissions
                        .last()
                        .and_then(|perm| permission_message_for_agent(agent, perm))
                    {
                        self.board_outbox.push_back(msg);
                    }
                    // Re-render live for passive sessions on focus so the
                    // PERM line shows up inside the terminal view too.
                    let agent_ref = self
                        .registered
                        .get(&(kind, perm.agent_id.clone()))
                        .expect("just updated");
                    if !agent_ref.from_launch
                        && agent_ref.board_sid.is_some()
                        && agent_ref.board_sid == self.focused_board_sid
                    {
                        if let Some(msg) = terminal_replay_message_for_agent(agent_ref) {
                            self.board_outbox.push_back(msg);
                        }
                    }
                    Ok(json!({"ok": true, "type": "permission.queued"}).to_string())
                } else {
                    Err(format!(
                        "no registered agent for permission: {}/{}",
                        kind.as_str(),
                        perm.agent_id
                    ))
                }
            }
            "permission.resolve" => {
                let resolve = request
                    .resolve
                    .ok_or_else(|| "permission.resolve requires resolve payload".to_string())?;
                let kind = parse_kind(&resolve.kind)?;
                if let Some(agent) = self.registered.get_mut(&(kind, resolve.agent_id.clone())) {
                    let before = agent.pending_permissions.len();
                    agent
                        .pending_permissions
                        .retain(|p| p.req_id != resolve.req_id);
                    let removed = before - agent.pending_permissions.len();
                    Ok(json!({
                        "ok": true,
                        "type": "permission.resolved",
                        "removed": removed,
                    })
                    .to_string())
                } else {
                    Err(format!(
                        "no registered agent for resolve: {}/{}",
                        kind.as_str(),
                        resolve.agent_id
                    ))
                }
            }
            "permission.poll" => {
                let poll = request
                    .poll
                    .ok_or_else(|| "permission.poll requires poll payload".to_string())?;
                let kind = parse_kind(&poll.kind)?;
                if let Some(agent) = self.registered.get_mut(&(kind, poll.agent_id.clone())) {
                    if let Some(decision) = agent.resolved_permissions.remove(&poll.req_id) {
                        Ok(json!({
                            "ok": true,
                            "type": "permission.decision",
                            "reqId": poll.req_id,
                            "decision": decision.as_str(),
                        })
                        .to_string())
                    } else {
                        let pending = agent
                            .pending_permissions
                            .iter()
                            .any(|perm| perm.req_id == poll.req_id);
                        Ok(json!({
                            "ok": true,
                            "type": "permission.pending",
                            "reqId": poll.req_id,
                            "pending": pending,
                        })
                        .to_string())
                    }
                } else {
                    Err(format!(
                        "no registered agent for poll: {}/{}",
                        kind.as_str(),
                        poll.agent_id
                    ))
                }
            }
            "terminal.stream" => {
                let stream = request
                    .stream
                    .ok_or_else(|| "terminal.stream requires stream payload".to_string())?;
                let kind = parse_kind(&stream.kind)?;
                let bytes = hex_decode(&stream.data_hex)
                    .ok_or_else(|| "terminal.stream dataHex invalid".to_string())?;
                let key = (kind, stream.agent_id.clone());
                if !self.registered.contains_key(&key) {
                    return Err(format!(
                        "no registered agent for terminal stream: {}/{}",
                        kind.as_str(),
                        stream.agent_id
                    ));
                };
                if !bytes.is_empty() {
                    let mut mirror_keys = vec![key.clone()];
                    mirror_keys.extend(
                        self.registered
                            .iter()
                            .filter(|(_, agent)| agent.attached_terminal.as_ref() == Some(&key))
                            .map(|(attached_key, _)| attached_key.clone()),
                    );
                    for mirror_key in mirror_keys {
                        let Some(agent) = self.registered.get_mut(&mirror_key) else {
                            continue;
                        };
                        agent.append_terminal_bytes(&bytes);
                        if agent.board_sid.is_some() && agent.board_sid == self.focused_board_sid {
                            self.board_outbox.push_back(HidMessage {
                                cmd: Cmd::Vt100Stream,
                                sid: agent.board_sid.unwrap().raw(),
                                payload: bytes.clone(),
                            });
                        }
                    }
                }
                Ok(json!({"ok": true, "type": "terminal.stream.accepted"}).to_string())
            }
            "session.abort" => {
                let abort = request
                    .abort
                    .ok_or_else(|| "session.abort requires abort payload".to_string())?;
                let kind = parse_kind(&abort.kind)?;
                // Drop the agent from `registered` so the periodic
                // SessionHeartbeat thread stops pinging this sid. After 3
                // missed heartbeats the board grays the row; after the
                // additional grace window it removes the row entirely. This
                // is the agreed liveness model — daemon-side is "stop
                // beating", board-side is "fade + drop".
                if let Some(_agent) = self.registered.remove(&(kind, abort.agent_id.clone())) {
                    // Also drain any pending board-session-request slot so
                    // the next RequestSession we send isn't matched against
                    // an agent we just dropped.
                    self.pending_board_sessions
                        .retain(|key| key != &(kind, abort.agent_id.clone()));
                    Ok(json!({"ok": true, "type": "session.abort.accepted"}).to_string())
                } else {
                    Err(format!(
                        "no registered agent for abort: {}/{}",
                        kind.as_str(),
                        abort.agent_id
                    ))
                }
            }
            other => Err(format!("unknown message type: {other}")),
        }
    }

    /// 测试与外部调用方查看一个 registered agent 的最新状态。
    pub fn get_agent(&self, kind: AgentKind, agent_id: &str) -> Option<&RegisteredAgent> {
        self.registered.get(&(kind, agent_id.to_string()))
    }

    /// Passive discovery 灌入 transcript 中已记录的对话 turn — **只更新 in-memory
    /// `agent.turns` 缓冲, 不 push 到板端 outbox**. 板端 terminal view 的"高度
    /// 一致"由 `start`-launched session 的 `terminal.stream` (Vt100Stream) 路径
    /// 负责; passive session 只在 grid 出现 SID, 不灌历史 TURN_APPEND, 否则会
    /// 把几 MB 的 transcript 历史一次性 dump 到板端形成一团文字。
    pub fn ingest_discovered_turns(&mut self, turns: &[DiscoveredTurn]) -> usize {
        let mut ingested = 0;
        for turn in turns {
            let key = (turn.kind, turn.agent_id.clone());
            let Some(agent) = self.registered.get_mut(&key) else {
                continue;
            };
            let sid = agent.board_sid.unwrap_or(BoardSid::BROADCAST);
            let event = ConversationTurn {
                sid,
                role: turn.role,
                text: turn.text.clone(),
                ts_ms: 0,
            };
            agent.turns.push(event);
            let cap = max_retained_turns_per_agent();
            if agent.turns.len() > cap {
                let overflow = agent.turns.len() - cap;
                agent.turns.drain(..overflow);
            }
            ingested += 1;
        }
        ingested
    }

    /// Drop transcript-only passive agents that are no longer in the latest
    /// discovery snapshot. Agents registered by hook/launch own their lifetime
    /// through `session.abort`, and unbound agents must not be pruned while a
    /// REQUEST_SESSION may still be queued for HID reconnect.
    pub fn prune_missing_passive_sessions(
        &mut self,
        latest: &[AgentSession],
    ) -> Vec<(AgentKind, String)> {
        use std::collections::HashSet;
        let seen: HashSet<(AgentKind, String)> = latest
            .iter()
            .map(|s| (s.kind, s.agent_id.clone()))
            .collect();
        for key in &seen {
            self.passive_missing_counts.remove(key);
        }
        let candidates: Vec<(AgentKind, String)> = self
            .registered
            .iter()
            .filter(|(_, agent)| {
                !agent.from_launch && !agent.from_hook && agent.board_sid.is_some()
            })
            .map(|(key, _)| key.clone())
            .collect();
        let mut to_drop = Vec::new();
        for key in candidates {
            if seen.contains(&key) {
                continue;
            }
            if self.passive_missing_counts.contains_key(&key) {
                let count = self.passive_missing_counts.entry(key.clone()).or_insert(0);
                *count = count.saturating_add(1);
                if *count >= PASSIVE_MISSING_GRACE_POLLS {
                    to_drop.push(key);
                }
            } else if self.registered.contains_key(&key) {
                self.passive_missing_counts.insert(key, 1);
            }
        }
        for key in &to_drop {
            self.registered.remove(key);
            self.pending_board_sessions.retain(|pending| pending != key);
            self.passive_missing_counts.remove(key);
        }
        to_drop
    }

    /// Passive discovery 入口: 把外部 poller 发现的 AgentSession 自动落成
    /// `RegisteredAgent`。已经在 `registered` 里的 key 直接跳过, 避免和 hook
    /// 路径互相覆盖。新 agent 会走 `enqueue_board_session_request`, 让板端
    /// grid 立刻收到 `REQUEST_SESSION`。返回新登记数。
    pub fn register_discovered_sessions(&mut self, sessions: &[AgentSession]) -> usize {
        let mut newly_registered = 0;
        for session in sessions {
            let key = (session.kind, session.agent_id.clone());
            if self.registered.contains_key(&key) {
                self.passive_missing_counts.remove(&key);
                continue;
            }
            let agent = RegisteredAgent::from_session(session);
            self.activities.push(AgentActivity {
                agent_id: agent.agent_id.clone(),
                kind: agent.kind,
                activity: AgentActivityKind::Seen,
                status: agent.status,
                transcript_path: session.transcript_path.clone(),
            });
            self.enqueue_board_session_request(&agent);
            self.passive_missing_counts.remove(&key);
            self.registered.insert(key, agent);
            newly_registered += 1;
        }
        newly_registered
    }

    pub fn drain_board_outbox(&mut self) -> Vec<HidMessage> {
        self.board_outbox.drain(..).collect()
    }

    pub fn handle_board_message(&mut self, msg: HidMessage) -> Result<(), String> {
        match msg.cmd {
            Cmd::SessionResponse => self.handle_session_response(msg.sid, msg.payload),
            Cmd::SessionFocus => {
                self.focus_board_sid(msg.sid);
                Ok(())
            }
            Cmd::PermissionRes => self.handle_permission_response(msg.sid, msg.payload),
            Cmd::SessionInvalid => self.handle_session_invalid(msg.sid, msg.payload),
            // Board-local UI commands the daemon does not need to act on. Notably
            // `EncoderEvent` is consumed by `aikb_lcd_ui` itself for picker / scroll
            // navigation; surfacing it here as an error would just spam stderr.
            // `KeyEvent` is similar — board firmware decides whether to act on it.
            Cmd::EncoderEvent | Cmd::KeyEvent => Ok(()),
            _ => Err(format!(
                "unsupported board message for daemon binding: {:?}",
                msg.cmd
            )),
        }
    }

    fn enqueue_board_session_request(&mut self, agent: &RegisteredAgent) {
        let key = (agent.kind, agent.agent_id.clone());
        if self
            .pending_board_sessions
            .iter()
            .any(|pending| pending == &key)
        {
            return;
        }
        self.pending_board_sessions.push_back(key);
        let hint = if agent.name.is_empty() {
            agent.agent_id.as_bytes().to_vec()
        } else {
            agent.name.as_bytes().to_vec()
        };
        eprintln!(
            "[board] request session for {}/{} hint={}",
            agent.kind.as_str(),
            agent.agent_id,
            String::from_utf8_lossy(&hint)
        );
        self.board_outbox.push_back(HidMessage {
            cmd: Cmd::RequestSession,
            sid: BoardSid::BROADCAST.raw(),
            payload: hint
                .into_iter()
                .take(vb_protocol::PLUGIN_HINT_MAX)
                .collect(),
        });
    }

    fn handle_session_response(&mut self, sid: u16, payload: Vec<u8>) -> Result<(), String> {
        if sid == 0 {
            return Err("SESSION_RESPONSE with sid=0".to_string());
        }
        let status = payload
            .first()
            .copied()
            .unwrap_or(SessionStatusByte::Ok as u8);
        if status != SessionStatusByte::Ok as u8 && status != SessionStatusByte::Created as u8 {
            return Err(format!("SESSION_RESPONSE failed status=0x{status:02x}"));
        }
        let key = self
            .pending_board_sessions
            .pop_front()
            .ok_or_else(|| "SESSION_RESPONSE without pending agent".to_string())?;
        let board_sid = BoardSid::new(sid);
        let old_sid = self.registered.get(&key).and_then(|agent| agent.board_sid);
        self.clear_board_sid_owners(board_sid, Some(&key));
        let Some(agent) = self.registered.get_mut(&key) else {
            return Err("pending agent disappeared before SESSION_RESPONSE".to_string());
        };
        agent.board_sid = Some(board_sid);
        if old_sid.is_some() && self.focused_board_sid == old_sid {
            self.focused_board_sid = Some(board_sid);
        }
        eprintln!(
            "[board] session response sid={} for {}/{}",
            sid,
            agent.kind.as_str(),
            agent.agent_id
        );
        for msg in board_sync_messages(agent) {
            self.board_outbox.push_back(msg);
        }
        Ok(())
    }

    fn handle_session_invalid(&mut self, sid: u16, payload: Vec<u8>) -> Result<(), String> {
        if sid == BoardSid::BROADCAST.raw() {
            let status = payload.first().copied().unwrap_or(0);
            eprintln!("[board] ignored broadcast session invalid status=0x{status:02x}");
            return Ok(());
        }
        let board_sid = BoardSid::new(sid);
        let detached = self.clear_board_sid_owners(board_sid, None);
        let status = payload.first().copied().unwrap_or(0);
        if detached == 0 {
            eprintln!("[board] session invalid sid={sid} status=0x{status:02x} had no owner");
        }
        Ok(())
    }

    fn clear_board_sid_owners(
        &mut self,
        sid: BoardSid,
        keep: Option<&(AgentKind, String)>,
    ) -> usize {
        let stale_keys: Vec<(AgentKind, String)> = self
            .registered
            .iter()
            .filter(|(key, agent)| {
                agent.board_sid == Some(sid) && keep.map_or(true, |keep_key| *key != keep_key)
            })
            .map(|(key, _)| key.clone())
            .collect();

        for key in &stale_keys {
            if let Some(agent) = self.registered.get_mut(key) {
                eprintln!(
                    "[board] detach sid={} from stale {}/{}",
                    sid.raw(),
                    agent.kind.as_str(),
                    agent.agent_id
                );
                agent.board_sid = None;
            }
        }
        if keep.is_none() && self.focused_board_sid == Some(sid) {
            self.focused_board_sid = None;
        }
        stale_keys.len()
    }

    fn focus_board_sid(&mut self, sid: u16) {
        if sid == 0 {
            return;
        }
        let board_sid = BoardSid::new(sid);
        self.focused_board_sid = Some(board_sid);
        let Some(key) = self
            .registered
            .iter()
            .find_map(|(key, agent)| (agent.board_sid == Some(board_sid)).then(|| key.clone()))
        else {
            eprintln!("[board] focus sid={} ignored: no registered agent", sid);
            return;
        };
        let replay = self.terminal_replay_message_for_key(&key);
        if let Some(agent) = self.registered.get(&key) {
            eprintln!(
                "[board] focus sid={} -> {}/{} attached={} buffered={} replay={}",
                sid,
                agent.kind.as_str(),
                agent.agent_id,
                agent.attached_terminal.is_some(),
                agent.terminal_buffer_len,
                replay.as_ref().map(|msg| msg.payload.len()).unwrap_or(0)
            );
        }
        if let Some(msg) = replay {
            self.board_outbox.push_back(msg);
        }
    }

    fn terminal_replay_message_for_key(&self, key: &(AgentKind, String)) -> Option<HidMessage> {
        let agent = self.registered.get(key)?;
        terminal_replay_message_for_agent(agent)
    }

    fn handle_permission_response(&mut self, sid: u16, payload: Vec<u8>) -> Result<(), String> {
        if sid == 0 {
            return Err("PERMISSION_RES with sid=0".to_string());
        }
        let (req_id, decision_byte) = decode_permission_response(&payload)
            .ok_or_else(|| "invalid PERMISSION_RES payload".to_string())?;
        let decision = permission_decision_from_byte(decision_byte);
        let Some(agent) = self
            .registered
            .values_mut()
            .find(|agent| agent.board_sid == Some(BoardSid::new(sid)))
        else {
            return Err(format!("PERMISSION_RES for unknown sid={sid}"));
        };
        agent
            .pending_permissions
            .retain(|permission| permission.req_id != req_id);
        agent.resolved_permissions.insert(req_id, decision);
        Ok(())
    }
}

fn parse_kind(value: &str) -> Result<AgentKind, String> {
    let kind = AgentKind::from_label(value);
    if kind == AgentKind::Unknown {
        return Err(format!("unknown agent kind: {value}"));
    }
    Ok(kind)
}

fn board_sync_messages(agent: &RegisteredAgent) -> Vec<HidMessage> {
    let mut out = Vec::new();
    if let Some(msg) = meta_message_for_agent(agent) {
        out.push(msg);
    }
    if let Some(msg) = status_message_for_agent(agent) {
        out.push(msg);
    }
    if let Some(msg) = token_message_for_agent(agent) {
        out.push(msg);
    }
    // Last turn used to be replayed here as a picker-row preview. Skipped for
    // passive agents: their TURN_APPEND would leak conversation text into the
    // picker row. Launched agents already drive their preview through
    // terminal.stream.
    if agent.from_launch {
        if let Some(turn) = agent.turns.last() {
            if let Some(msg) = turn_message_for_agent(agent, turn) {
                out.push(msg);
            }
        }
    }
    for perm in &agent.pending_permissions {
        if let Some(msg) = permission_message_for_agent(agent, perm) {
            out.push(msg);
        }
    }
    out
}

fn is_transport_only_terminal(agent: &RegisteredAgent) -> bool {
    agent.kind == AgentKind::Terminal && agent.from_launch
}

fn meta_message_for_agent(agent: &RegisteredAgent) -> Option<HidMessage> {
    let sid = agent.board_sid?.raw();
    Some(HidMessage {
        cmd: Cmd::AgentMeta,
        sid,
        payload: encode_agent_meta(agent_kind_byte(agent.kind), &agent.cwd, ""),
    })
}

fn status_message_for_agent(agent: &RegisteredAgent) -> Option<HidMessage> {
    let sid = agent.board_sid?.raw();
    Some(HidMessage {
        cmd: Cmd::StatusUpdate,
        sid,
        payload: vec![status_byte(agent.status) as u8],
    })
}

fn token_message_for_agent(agent: &RegisteredAgent) -> Option<HidMessage> {
    if agent.usage == TokenUsage::default() {
        return None;
    }
    let sid = agent.board_sid?.raw();
    Some(HidMessage {
        cmd: Cmd::TokenUsage,
        sid,
        payload: encode_token_usage(agent.usage),
    })
}

fn turn_message_for_agent(agent: &RegisteredAgent, turn: &ConversationTurn) -> Option<HidMessage> {
    let sid = agent.board_sid?.raw();
    Some(HidMessage {
        cmd: Cmd::TurnAppend,
        sid,
        payload: encode_turn_append(turn_role_byte(turn.role), &turn.text),
    })
}

fn permission_message_for_agent(
    agent: &RegisteredAgent,
    permission: &PermissionRequest,
) -> Option<HidMessage> {
    let sid = agent.board_sid?.raw();
    Some(HidMessage {
        cmd: Cmd::PermissionReq,
        sid,
        payload: encode_permission_request(
            permission.req_id,
            &permission.tool,
            &permission.args_summary,
        ),
    })
}

fn terminal_replay_message_for_agent(agent: &RegisteredAgent) -> Option<HidMessage> {
    let sid = agent.board_sid?.raw();
    let snapshot = agent.terminal_snapshot();
    if !snapshot.is_empty() {
        let mut payload = Vec::with_capacity(SCREEN_CLEAR.len() + snapshot.len());
        payload.extend_from_slice(SCREEN_CLEAR);
        payload.extend_from_slice(&snapshot);
        return Some(HidMessage {
            cmd: Cmd::Vt100Stream,
            sid,
            payload,
        });
    }
    if agent.attached_terminal.is_some() {
        return None;
    }
    if agent.from_launch {
        return Some(HidMessage {
            cmd: Cmd::Vt100Stream,
            sid,
            payload: SCREEN_CLEAR.to_vec(),
        });
    }
    let mut text = String::new();
    text.push_str(std::str::from_utf8(SCREEN_CLEAR).unwrap_or("\x1b[2J\x1b[H"));
    text.push_str(&render_passive_placeholder(agent));
    Some(HidMessage {
        cmd: Cmd::Vt100Stream,
        sid,
        payload: text.into_bytes(),
    })
}

fn render_passive_placeholder(agent: &RegisteredAgent) -> String {
    const RESET: &str = "\x1b[0m";
    const BOLD: &str = "\x1b[1m";
    const DIM: &str = "\x1b[90m"; // bright black = dim grey
    const FG_KIND: &str = "\x1b[36m"; // cyan
    const FG_NAME: &str = "\x1b[33m"; // yellow / orange

    let name = if agent.name.is_empty() {
        agent.agent_id.as_str()
    } else {
        agent.name.as_str()
    };
    format!(
        "{BOLD}{FG_KIND}{}{RESET}  {FG_NAME}{name}{RESET}\r\n\
         {DIM}No live terminal capture for this passive session.{RESET}\r\n\
         {DIM}Open a Vibe-captured Windows Terminal profile for 1:1 replay.{RESET}\r\n",
        agent.kind.as_str(),
    )
}

fn agent_kind_byte(kind: AgentKind) -> u8 {
    match kind {
        AgentKind::Claude => 0,
        AgentKind::Codex => 1,
        AgentKind::Terminal => 0xFF,
        AgentKind::Unknown => 0xFF,
    }
}

fn status_byte(status: AgentStatus) -> SessionStateByte {
    match status {
        AgentStatus::Running => SessionStateByte::Run,
        AgentStatus::WaitingInput => SessionStateByte::Wait,
        AgentStatus::Idle => SessionStateByte::Connected,
        AgentStatus::Done => SessionStateByte::Done,
        AgentStatus::Error => SessionStateByte::Error,
        AgentStatus::Unknown => SessionStateByte::Connected,
    }
}

fn turn_role_byte(role: TurnRole) -> TurnRoleByte {
    match role {
        TurnRole::User => TurnRoleByte::User,
        TurnRole::Assistant => TurnRoleByte::Assistant,
        TurnRole::Tool => TurnRoleByte::Tool,
        TurnRole::System => TurnRoleByte::System,
    }
}

fn permission_decision_from_byte(decision: PermissionDecisionByte) -> PermissionDecision {
    match decision {
        PermissionDecisionByte::Allow => PermissionDecision::Allow,
        PermissionDecisionByte::Deny => PermissionDecision::Deny,
        PermissionDecisionByte::Always => PermissionDecision::Always,
    }
}

fn parse_role(value: &str) -> Result<TurnRole, String> {
    Ok(match value.trim().to_ascii_lowercase().as_str() {
        "user" => TurnRole::User,
        "assistant" | "model" => TurnRole::Assistant,
        "tool" | "tool_use" | "tool_result" => TurnRole::Tool,
        "system" => TurnRole::System,
        other => return Err(format!("unknown turn role: {other}")),
    })
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

    fn from_session(session: &AgentSession) -> Self {
        Self {
            agent_id: session.agent_id.clone(),
            kind: session.kind,
            name: session.name.clone(),
            cwd: session.cwd.clone(),
            focus_target: None,
            terminal_hwnd: session.terminal_hwnd,
            status: session.status,
            board_sid: None,
            usage: TokenUsage::default(),
            turns: Vec::new(),
            pending_permissions: Vec::new(),
            resolved_permissions: HashMap::new(),
            from_hook: false,
            from_launch: false,
            terminal_buffer: VecDeque::new(),
            terminal_buffer_len: 0,
            attached_terminal: None,
        }
    }

    fn append_terminal_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if bytes.len() >= VT100_BUFFER_BYTES {
            self.terminal_buffer.clear();
            let keep_from = bytes.len() - VT100_BUFFER_BYTES;
            self.terminal_buffer.push_back(bytes[keep_from..].to_vec());
            self.terminal_buffer_len = VT100_BUFFER_BYTES;
            return;
        }
        self.terminal_buffer.push_back(bytes.to_vec());
        self.terminal_buffer_len += bytes.len();
        while self.terminal_buffer_len > VT100_BUFFER_BYTES {
            let Some(front) = self.terminal_buffer.front_mut() else {
                self.terminal_buffer_len = 0;
                return;
            };
            let overflow = self.terminal_buffer_len - VT100_BUFFER_BYTES;
            if front.len() <= overflow {
                let removed = self.terminal_buffer.pop_front().unwrap();
                self.terminal_buffer_len -= removed.len();
            } else {
                front.drain(..overflow);
                self.terminal_buffer_len -= overflow;
            }
        }
    }

    fn terminal_snapshot(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.terminal_buffer_len);
        for chunk in &self.terminal_buffer {
            out.extend_from_slice(chunk);
        }
        out
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistrationEnvelope {
    #[serde(rename = "type")]
    message_type: String,
    agent: Option<RegistrationAgentPayload>,
    activity: Option<ActivityPayload>,
    token: Option<TokenUpdatePayload>,
    turn: Option<TurnPayload>,
    permission: Option<PermissionRequestPayload>,
    resolve: Option<PermissionResolvePayload>,
    poll: Option<PermissionPollPayload>,
    abort: Option<AbortPayload>,
    stream: Option<TerminalStreamPayload>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalStreamPayload {
    agent_id: String,
    kind: String,
    /// Raw ConPTY stdout bytes encoded as lowercase hex. Hex over JSON is
    /// 2x overhead but avoids pulling in a base64 dependency and keeps the
    /// rest of the registration protocol uniformly text.
    data_hex: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenUpdatePayload {
    agent_id: String,
    kind: String,
    #[serde(default)]
    input: u64,
    #[serde(default)]
    output: u64,
    #[serde(default)]
    cost_cents: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TurnPayload {
    agent_id: String,
    kind: String,
    role: String,
    text: String,
    #[serde(default)]
    ts_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PermissionRequestPayload {
    agent_id: String,
    kind: String,
    req_id: u64,
    tool: String,
    #[serde(default)]
    args_summary: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PermissionResolvePayload {
    agent_id: String,
    kind: String,
    req_id: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PermissionPollPayload {
    agent_id: String,
    kind: String,
    req_id: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AbortPayload {
    agent_id: String,
    kind: String,
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
    #[serde(default)]
    parent_agent_id: Option<String>,
    #[serde(default)]
    parent_kind: Option<String>,
    /// True only when sent by `vb-daemon launch`/`start` (daemon owns the
    /// child ConPTY). Hook adapter omits it → defaults false → board replay
    /// stays empty for that agent. Hook may still update other fields by
    /// re-registering with the same agentId; it cannot promote a passive
    /// agent to launched (we keep the existing flag if already true).
    #[serde(default)]
    from_launch: bool,
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
        let attached_terminal = match (self.parent_kind.as_deref(), self.parent_agent_id) {
            (Some(parent_kind), Some(parent_agent_id)) if !parent_agent_id.trim().is_empty() => {
                let parent_kind = AgentKind::from_label(parent_kind);
                if parent_kind == AgentKind::Unknown {
                    return Err(format!("unknown parent kind: {parent_kind:?}"));
                }
                Some((parent_kind, parent_agent_id))
            }
            _ => None,
        };
        Ok(RegisteredAgent {
            agent_id: self.agent_id,
            kind,
            name: self.name,
            cwd: self.cwd,
            focus_target: self.focus_target,
            terminal_hwnd: self.terminal_hwnd,
            status: status_from_label(self.status.as_deref().unwrap_or("running")),
            board_sid: None,
            usage: TokenUsage::default(),
            turns: Vec::new(),
            pending_permissions: Vec::new(),
            resolved_permissions: HashMap::new(),
            from_hook: true,
            from_launch: self.from_launch,
            terminal_buffer: VecDeque::new(),
            terminal_buffer_len: 0,
            attached_terminal,
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

fn passive_board_discovery_enabled() -> bool {
    env_flag_enabled(
        std::env::var("VIBE_BRIDGE_PASSIVE_DISCOVERY")
            .ok()
            .as_deref(),
    )
}

fn env_flag_enabled(value: Option<&str>) -> bool {
    matches!(
        value.map(|value| value.trim().to_ascii_lowercase()),
        Some(value) if matches!(value.as_str(), "1" | "true" | "yes" | "on")
    )
}

pub fn run_tcp_hid_daemon<A, T>(addr: A, hid: Arc<T>) -> Result<(), String>
where
    A: ToSocketAddrs,
    T: HidTransport + Send + Sync + 'static,
{
    let listener = TcpListener::bind(addr).map_err(|err| format!("bind hid daemon: {err}"))?;
    let daemon = Arc::new(Mutex::new(BridgeDaemon::new()));

    {
        let daemon = Arc::clone(&daemon);
        let hid = Arc::clone(&hid);
        thread::spawn(move || {
            let mut last_read_error = String::new();
            loop {
                match hid.recv() {
                    Ok(msg) => {
                        last_read_error.clear();
                        if let Ok(mut daemon) = daemon.lock() {
                            if let Err(err) = daemon.handle_board_message(msg) {
                                eprintln!("board message ignored: {err}");
                            }
                            if let Err(err) = flush_board_outbox(&mut daemon, hid.as_ref()) {
                                eprintln!("board outbox flush failed: {err}");
                            }
                        }
                    }
                    Err(err) => {
                        let message = err.to_string();
                        if message != last_read_error {
                            eprintln!("HID read failed: {message}");
                            last_read_error = message;
                        }
                        thread::sleep(Duration::from_secs(1));
                    }
                }
            }
        });
    }

    {
        let daemon = Arc::clone(&daemon);
        let hid = Arc::clone(&hid);
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(2));
            if !hid.is_connected() {
                continue;
            }
            if let Ok(mut daemon) = daemon.lock() {
                // Host-level heartbeat: broadcast sid lets the board distinguish
                // "bridge daemon alive" from per-session liveness. Existing
                // per-session heartbeat stays below for compatibility.
                daemon.board_outbox.push_back(HidMessage {
                    cmd: Cmd::SessionHeartbeat,
                    sid: BoardSid::BROADCAST.raw(),
                    payload: Vec::new(),
                });
                let sids: Vec<u16> = daemon
                    .registered
                    .values()
                    .filter_map(|agent| agent.board_sid.map(|sid| sid.raw()))
                    .collect();
                for sid in sids {
                    daemon.board_outbox.push_back(HidMessage {
                        cmd: Cmd::SessionHeartbeat,
                        sid,
                        payload: Vec::new(),
                    });
                }
                if let Err(err) = flush_board_outbox(&mut daemon, hid.as_ref()) {
                    eprintln!("heartbeat flush failed: {err}");
                }
            }
        });
    }

    if passive_board_discovery_enabled() {
        // Passive discovery 线程: 周期性扫 Claude/Codex transcript (含 WSL home),
        // 把新看到的 session 自动登记到 daemon, 让板端 grid 出现 SID, 而不依赖
        // Claude Code 主动 hook 上报。Poller 留在线程内, 长耗时的 wsl.exe/UIA
        // 调用不持 daemon 锁。
        //
        // 始终走 `poll_once` (含 active filter)。早期版本第一轮用
        // `poll_candidates_once` 把所有历史 transcript 都推到板端 grid,
        // 实测下 grid 会瞬间出现十几条历史会话, 用户体验差。filter 现在做了
        // multi-signal union (active process + transcript Running/Idle + 10min
        // 内修改), 第一轮就能拿到真实在跑的 agent。
        let daemon = Arc::clone(&daemon);
        let hid = Arc::clone(&hid);
        thread::spawn(move || {
            let mut poller = AgentSourcePoller::new();
            loop {
                let snapshot = poller.poll_once();
                let snapshot = match snapshot {
                    Ok(s) => s,
                    Err(err) => {
                        eprintln!("passive discovery poll failed: {err}");
                        thread::sleep(Duration::from_millis(1500));
                        continue;
                    }
                };
                if let Ok(mut daemon) = daemon.lock() {
                    let added = daemon.register_discovered_sessions(&snapshot.sessions);
                    if added > 0 {
                        eprintln!("passive discovery: registered {added} new session(s)");
                    }
                    let ingested = daemon.ingest_discovered_turns(&snapshot.turns);
                    if ingested > 0 {
                        eprintln!("passive discovery: ingested {ingested} turn(s)");
                    }
                    // Drop passive agents that fell off the latest snapshot —
                    // closed claude/codex windows, stale transcripts. Their
                    // heartbeats stop immediately; the board side fades and
                    // drops the row per the agreed liveness model.
                    let dropped = daemon.prune_missing_passive_sessions(&snapshot.sessions);
                    if !dropped.is_empty() {
                        eprintln!(
                            "passive discovery: pruned {} stale session(s)",
                            dropped.len()
                        );
                    }
                    if let Err(err) = flush_board_outbox(&mut daemon, hid.as_ref()) {
                        eprintln!("passive discovery flush failed: {err}");
                    }
                }
                thread::sleep(Duration::from_millis(1500));
            }
        });
    } else {
        eprintln!(
            "passive discovery: disabled for board UI; set VIBE_BRIDGE_PASSIVE_DISCOVERY=1 to re-enable transcript fallback"
        );
    }

    for stream in listener.incoming() {
        let stream = stream.map_err(|err| format!("accept registration client: {err}"))?;
        let daemon = Arc::clone(&daemon);
        let hid = Arc::clone(&hid);
        thread::spawn(move || {
            if let Err(err) = handle_client_shared(stream, &daemon, hid.as_ref()) {
                eprintln!("registration client dropped: {err}");
            }
        });
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
    loop {
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(err) if is_disconnect_error(&err) => break,
            Err(err) => return Err(format!("read registration line: {err}")),
        }
        let response = match daemon.handle_json_line(line.trim()) {
            Ok(response) => response,
            Err(err) => json!({"ok": false, "error": err}).to_string(),
        };
        if let Err(err) = writeln!(stream, "{response}") {
            if is_disconnect_error(&err) {
                break;
            }
            return Err(format!("write registration response: {err}"));
        }
        line.clear();
    }
    Ok(())
}

fn handle_client_shared<T: HidTransport>(
    mut stream: TcpStream,
    daemon: &Arc<Mutex<BridgeDaemon>>,
    hid: &T,
) -> Result<(), String> {
    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|err| format!("clone registration stream: {err}"))?,
    );
    let mut line = String::new();
    loop {
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(err) if is_disconnect_error(&err) => break,
            Err(err) => return Err(format!("read registration line: {err}")),
        }
        let response = {
            let mut daemon = daemon
                .lock()
                .map_err(|_| "daemon mutex poisoned".to_string())?;
            match daemon.handle_json_line(line.trim()) {
                Ok(response) => response,
                Err(err) => json!({"ok": false, "error": err}).to_string(),
            }
        };
        if let Err(err) = writeln!(stream, "{response}") {
            if is_disconnect_error(&err) {
                break;
            }
            return Err(format!("write registration response: {err}"));
        }
        if let Ok(mut daemon) = daemon.lock() {
            if let Err(err) = flush_board_outbox(&mut daemon, hid) {
                eprintln!("board outbox flush failed: {err}");
            }
        }
        line.clear();
    }
    Ok(())
}

fn is_disconnect_error(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::BrokenPipe
            | ErrorKind::UnexpectedEof
    )
}

fn flush_board_outbox<T: HidTransport>(daemon: &mut BridgeDaemon, hid: &T) -> Result<(), String> {
    if !hid.is_connected() {
        return Ok(());
    }
    while let Some(msg) = daemon.board_outbox.pop_front() {
        let frames = split_host_to_board(msg.cmd, msg.sid, &msg.payload);
        let frame_count = frames.len();
        for frame in frames {
            if let Err(err) = hid.send(&HidMessage {
                cmd: frame.cmd,
                sid: frame.sid,
                payload: frame.payload,
            }) {
                let send_err = format!("HID send {:?}/sid {}: {err}", msg.cmd, msg.sid);
                daemon.board_outbox.push_front(msg);
                return Err(send_err);
            }
        }
        if msg.cmd == Cmd::Vt100Stream {
            eprintln!(
                "[board] sent vt100 sid={} bytes={} frames={}",
                msg.sid,
                msg.payload.len(),
                frame_count
            );
        }
    }
    Ok(())
}

/// Decode a lowercase/uppercase ASCII hex string into bytes. Returns `None`
/// on any invalid char or odd length. Used by the `terminal.stream` JSON
/// transport for ConPTY stdout chunks.
pub fn hex_decode(input: &str) -> Option<Vec<u8>> {
    if input.len() % 2 != 0 {
        return None;
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut idx = 0;
    while idx < bytes.len() {
        let hi = hex_nibble(bytes[idx])?;
        let lo = hex_nibble(bytes[idx + 1])?;
        out.push((hi << 4) | lo);
        idx += 2;
    }
    Some(out)
}

/// Encode bytes as lowercase hex. Mirror of [`hex_decode`].
pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
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
    use vb_protocol::payloads::{
        decode_agent_meta, decode_token_usage, encode_permission_response,
    };
    use vb_transport::TransportError;

    struct FailingTransport {
        failures_remaining: Mutex<usize>,
        sent: Mutex<Vec<HidMessage>>,
    }

    impl FailingTransport {
        fn new(failures: usize) -> Self {
            Self {
                failures_remaining: Mutex::new(failures),
                sent: Mutex::new(Vec::new()),
            }
        }
    }

    impl HidTransport for FailingTransport {
        fn send(&self, msg: &HidMessage) -> Result<(), TransportError> {
            let mut failures = self.failures_remaining.lock().unwrap();
            if *failures > 0 {
                *failures -= 1;
                return Err(TransportError::Io("injected HID failure".to_string()));
            }
            self.sent.lock().unwrap().push(msg.clone());
            Ok(())
        }

        fn recv(&self) -> Result<HidMessage, TransportError> {
            Err(TransportError::Timeout)
        }

        fn is_connected(&self) -> bool {
            true
        }
    }

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
    fn register_agent_requests_board_session_then_binds_response() {
        let mut daemon = BridgeDaemon::new();
        daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"codex-1","kind":"codex","name":"AIKB","cwd":"/work/AIKB"}}"#,
            )
            .unwrap();

        let outbox = daemon.drain_board_outbox();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].cmd, Cmd::RequestSession);
        assert_eq!(outbox[0].sid, 0);
        assert_eq!(outbox[0].payload, b"AIKB");

        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 7,
                payload: vec![SessionStatusByte::Created as u8],
            })
            .unwrap();

        let agent = daemon.get_agent(AgentKind::Codex, "codex-1").unwrap();
        assert_eq!(agent.board_sid, Some(BoardSid::new(7)));
        let outbox = daemon.drain_board_outbox();
        assert_eq!(outbox[0].cmd, Cmd::AgentMeta);
        assert_eq!(outbox[0].sid, 7);
        let (kind, cwd, branch) = decode_agent_meta(&outbox[0].payload).unwrap();
        assert_eq!(kind, 1);
        assert_eq!(cwd, "/work/AIKB");
        assert_eq!(branch, "");
        assert_eq!(outbox[1].cmd, Cmd::StatusUpdate);
        assert_eq!(outbox[1].payload, vec![SessionStateByte::Run as u8]);
    }

    #[test]
    fn launched_terminal_is_transport_only_and_does_not_request_board_session() {
        let mut daemon = BridgeDaemon::new();
        daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"terminal-1","kind":"terminal","name":"PowerShell","cwd":"/work","fromLaunch":true}}"#,
            )
            .unwrap();

        assert!(daemon.drain_board_outbox().is_empty());
        assert!(daemon.pending_board_sessions.is_empty());
        assert_eq!(
            daemon
                .get_agent(AgentKind::Terminal, "terminal-1")
                .unwrap()
                .board_sid,
            None
        );
    }

    #[test]
    fn session_response_reassigns_reused_sid_from_stale_agent() {
        let mut daemon = BridgeDaemon::new();
        daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"claude-old","kind":"claude","name":"Claude","cwd":"/work","fromLaunch":true}}"#,
            )
            .unwrap();
        daemon.drain_board_outbox();
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 3,
                payload: vec![SessionStatusByte::Created as u8],
            })
            .unwrap();
        daemon.drain_board_outbox();
        daemon
            .registered
            .get_mut(&(AgentKind::Claude, "claude-old".to_string()))
            .unwrap()
            .append_terminal_bytes(b"old claude bytes");

        daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"codex-new","kind":"codex","name":"codex","cwd":"/work","fromLaunch":true}}"#,
            )
            .unwrap();
        daemon.drain_board_outbox();
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 3,
                payload: vec![SessionStatusByte::Created as u8],
            })
            .unwrap();
        daemon.drain_board_outbox();
        daemon
            .registered
            .get_mut(&(AgentKind::Codex, "codex-new".to_string()))
            .unwrap()
            .append_terminal_bytes(b"new codex bytes");

        assert_eq!(
            daemon
                .get_agent(AgentKind::Claude, "claude-old")
                .map(|agent| agent.board_sid)
                .flatten(),
            None
        );
        assert_eq!(
            daemon
                .get_agent(AgentKind::Codex, "codex-new")
                .unwrap()
                .board_sid,
            Some(BoardSid::new(3))
        );

        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionFocus,
                sid: 3,
                payload: Vec::new(),
            })
            .unwrap();
        let outbox = daemon.drain_board_outbox();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].cmd, Cmd::Vt100Stream);
        assert_eq!(outbox[0].sid, 3);
        assert!(outbox[0].payload.ends_with(b"new codex bytes"));
        assert!(!outbox[0].payload.ends_with(b"old terminal bytes"));
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

    fn register_claude(daemon: &mut BridgeDaemon) {
        daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"claude-x","kind":"claude","cwd":"/w"}}"#,
            )
            .unwrap();
    }

    #[test]
    fn token_update_accumulates_usage() {
        let mut daemon = BridgeDaemon::new();
        register_claude(&mut daemon);
        let resp = daemon
            .handle_json_line(
                r#"{"type":"token.update","token":{"agentId":"claude-x","kind":"claude","input":1000,"output":500,"costCents":7}}"#,
            )
            .unwrap();
        assert!(resp.contains("token.accepted"));
        let agent = daemon.get_agent(AgentKind::Claude, "claude-x").unwrap();
        assert_eq!(agent.usage.input, 1000);
        assert_eq!(agent.usage.output, 500);
        assert_eq!(agent.usage.cost_cents, 7);
    }

    #[test]
    fn token_update_after_board_bind_enqueues_hid_message() {
        let mut daemon = BridgeDaemon::new();
        register_claude(&mut daemon);
        daemon.drain_board_outbox();
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 3,
                payload: vec![SessionStatusByte::Created as u8],
            })
            .unwrap();
        daemon.drain_board_outbox();

        daemon
            .handle_json_line(
                r#"{"type":"token.update","token":{"agentId":"claude-x","kind":"claude","input":1000,"output":500,"costCents":7}}"#,
            )
            .unwrap();

        let outbox = daemon.drain_board_outbox();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].cmd, Cmd::TokenUsage);
        assert_eq!(outbox[0].sid, 3);
        assert_eq!(
            decode_token_usage(&outbox[0].payload),
            Some(TokenUsage {
                input: 1000,
                output: 500,
                cost_cents: 7,
            })
        );
    }

    #[test]
    fn session_focus_for_launched_agent_clears_screen_for_live_vt100() {
        // If a launched agent has not emitted any terminal.stream bytes yet,
        // focus still clears the stale screen. It must not fall back to
        // transcript turn text, because PTY bytes are the fidelity source.
        let mut daemon = BridgeDaemon::new();
        daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"claude-x","kind":"claude","cwd":"/w","fromLaunch":true}}"#,
            )
            .unwrap();
        daemon.drain_board_outbox();
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 5,
                payload: vec![SessionStatusByte::Created as u8],
            })
            .unwrap();
        daemon.drain_board_outbox();
        daemon
            .handle_json_line(
                r#"{"type":"turn.append","turn":{"agentId":"claude-x","kind":"claude","role":"user","text":"hello board","tsMs":100}}"#,
            )
            .unwrap();
        daemon.drain_board_outbox();

        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionFocus,
                sid: 5,
                payload: Vec::new(),
            })
            .unwrap();

        let outbox = daemon.drain_board_outbox();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].cmd, Cmd::Vt100Stream);
        assert_eq!(outbox[0].sid, 5);
        let text = String::from_utf8(outbox[0].payload.clone()).unwrap();
        assert!(
            text.starts_with("\x1b[2J\x1b[H"),
            "expected screen-clear escape, got {text:?}"
        );
        assert!(
            !text.contains("hello board"),
            "launched replay must not leak turn text, got {text:?}"
        );
    }

    #[test]
    fn repeated_register_preserves_board_sid_and_turns() {
        let mut daemon = BridgeDaemon::new();
        register_claude(&mut daemon);
        daemon.drain_board_outbox();
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 4,
                payload: vec![SessionStatusByte::Created as u8],
            })
            .unwrap();
        daemon.drain_board_outbox();
        daemon
            .handle_json_line(
                r#"{"type":"turn.append","turn":{"agentId":"claude-x","kind":"claude","role":"user","text":"keep me","tsMs":100}}"#,
            )
            .unwrap();
        daemon.drain_board_outbox();

        register_claude(&mut daemon);
        let agent = daemon.get_agent(AgentKind::Claude, "claude-x").unwrap();
        assert_eq!(agent.board_sid, Some(BoardSid::new(4)));
        assert_eq!(agent.turns.len(), 1);
        assert_eq!(agent.turns[0].text, "keep me");
        let outbox = daemon.drain_board_outbox();
        assert!(outbox.is_empty());
    }

    #[test]
    fn repeated_register_does_not_replay_stale_pending_permissions() {
        let mut daemon = BridgeDaemon::new();
        register_claude(&mut daemon);
        daemon.drain_board_outbox();
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 4,
                payload: vec![SessionStatusByte::Created as u8],
            })
            .unwrap();
        daemon.drain_board_outbox();
        daemon
            .handle_json_line(
                r#"{"type":"permission.request","permission":{"agentId":"claude-x","kind":"claude","reqId":1,"tool":"unknown","argsSummary":"{}"}}"#,
            )
            .unwrap();
        daemon.drain_board_outbox();

        register_claude(&mut daemon);
        assert!(daemon.drain_board_outbox().is_empty());

        daemon
            .handle_json_line(
                r#"{"type":"permission.request","permission":{"agentId":"claude-x","kind":"claude","reqId":2,"tool":"Bash","argsSummary":"{\"command\":\"cargo test\"}"}}"#,
            )
            .unwrap();
        let outbox = daemon.drain_board_outbox();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].cmd, Cmd::PermissionReq);
        let (_req_id, tool, args) =
            vb_protocol::payloads::decode_permission_request(&outbox[0].payload).unwrap();
        assert_eq!(tool, "Bash");
        assert!(args.contains("cargo test"));
    }

    #[test]
    fn failed_hid_flush_keeps_message_for_retry() {
        let mut daemon = BridgeDaemon::new();
        daemon.board_outbox.push_back(HidMessage {
            cmd: Cmd::RequestSession,
            sid: BoardSid::BROADCAST.raw(),
            payload: b"AIKB".to_vec(),
        });
        let hid = FailingTransport::new(1);

        let err = flush_board_outbox(&mut daemon, &hid).unwrap_err();
        assert!(err.contains("RequestSession"));
        assert_eq!(daemon.board_outbox.len(), 1);
        assert!(hid.sent.lock().unwrap().is_empty());

        flush_board_outbox(&mut daemon, &hid).unwrap();
        assert!(daemon.board_outbox.is_empty());
        let sent = hid.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].cmd, Cmd::RequestSession);
        assert_eq!(sent[0].sid, BoardSid::BROADCAST.raw());
        assert_eq!(sent[0].payload, b"AIKB");
    }

    #[test]
    fn board_session_invalid_clears_sid_owner() {
        let mut daemon = BridgeDaemon::new();
        register_claude(&mut daemon);
        daemon.drain_board_outbox();
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 7,
                payload: vec![SessionStatusByte::Created as u8],
            })
            .unwrap();
        daemon.drain_board_outbox();

        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionFocus,
                sid: 7,
                payload: Vec::new(),
            })
            .unwrap();
        daemon.drain_board_outbox();
        assert_eq!(daemon.focused_board_sid, Some(BoardSid::new(7)));

        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionInvalid,
                sid: 7,
                payload: vec![SessionStatusByte::InvalidS as u8],
            })
            .unwrap();

        assert_eq!(
            daemon
                .get_agent(AgentKind::Claude, "claude-x")
                .unwrap()
                .board_sid,
            None
        );
        assert_eq!(daemon.focused_board_sid, None);
    }

    #[test]
    fn broadcast_session_invalid_does_not_clear_bound_sid() {
        let mut daemon = BridgeDaemon::new();
        register_claude(&mut daemon);
        daemon.drain_board_outbox();
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 7,
                payload: vec![SessionStatusByte::Created as u8],
            })
            .unwrap();
        daemon.drain_board_outbox();

        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionInvalid,
                sid: BoardSid::BROADCAST.raw(),
                payload: vec![SessionStatusByte::InvalidS as u8],
            })
            .unwrap();

        assert_eq!(
            daemon
                .get_agent(AgentKind::Claude, "claude-x")
                .unwrap()
                .board_sid,
            Some(BoardSid::new(7))
        );
    }

    #[test]
    fn turn_append_records_history() {
        let mut daemon = BridgeDaemon::new();
        register_claude(&mut daemon);
        daemon
            .handle_json_line(
                r#"{"type":"turn.append","turn":{"agentId":"claude-x","kind":"claude","role":"user","text":"hi","tsMs":100}}"#,
            )
            .unwrap();
        daemon
            .handle_json_line(
                r#"{"type":"turn.append","turn":{"agentId":"claude-x","kind":"claude","role":"assistant","text":"yo","tsMs":200}}"#,
            )
            .unwrap();
        let agent = daemon.get_agent(AgentKind::Claude, "claude-x").unwrap();
        assert_eq!(agent.turns.len(), 2);
        assert_eq!(agent.turns[0].role, TurnRole::User);
        assert_eq!(agent.turns[0].text, "hi");
        assert_eq!(agent.turns[1].role, TurnRole::Assistant);
    }

    #[test]
    fn permission_request_then_resolve_clears_pending() {
        let mut daemon = BridgeDaemon::new();
        register_claude(&mut daemon);
        daemon
            .handle_json_line(
                r#"{"type":"permission.request","permission":{"agentId":"claude-x","kind":"claude","reqId":42,"tool":"Write","argsSummary":"main.rs"}}"#,
            )
            .unwrap();
        let agent = daemon.get_agent(AgentKind::Claude, "claude-x").unwrap();
        assert_eq!(agent.pending_permissions.len(), 1);
        assert_eq!(agent.pending_permissions[0].req_id, 42);
        assert_eq!(agent.pending_permissions[0].tool, "Write");

        let resp = daemon
            .handle_json_line(
                r#"{"type":"permission.resolve","resolve":{"agentId":"claude-x","kind":"claude","reqId":42}}"#,
            )
            .unwrap();
        assert!(resp.contains("permission.resolved"));
        assert!(resp.contains("\"removed\":1"));
        let agent = daemon.get_agent(AgentKind::Claude, "claude-x").unwrap();
        assert!(agent.pending_permissions.is_empty());
    }

    #[test]
    fn board_permission_response_is_polled_by_hook() {
        let mut daemon = BridgeDaemon::new();
        register_claude(&mut daemon);
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 4,
                payload: vec![SessionStatusByte::Created as u8],
            })
            .unwrap();
        daemon.drain_board_outbox();
        daemon
            .handle_json_line(
                r#"{"type":"permission.request","permission":{"agentId":"claude-x","kind":"claude","reqId":42,"tool":"Write","argsSummary":"main.rs"}}"#,
            )
            .unwrap();
        assert_eq!(
            daemon
                .get_agent(AgentKind::Claude, "claude-x")
                .unwrap()
                .pending_permissions
                .len(),
            1
        );

        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::PermissionRes,
                sid: 4,
                payload: encode_permission_response(42, PermissionDecisionByte::Deny),
            })
            .unwrap();
        let agent = daemon.get_agent(AgentKind::Claude, "claude-x").unwrap();
        assert!(agent.pending_permissions.is_empty());
        assert_eq!(
            agent.resolved_permissions.get(&42),
            Some(&PermissionDecision::Deny)
        );

        let resp = daemon
            .handle_json_line(
                r#"{"type":"permission.poll","poll":{"agentId":"claude-x","kind":"claude","reqId":42}}"#,
            )
            .unwrap();
        assert!(resp.contains("\"type\":\"permission.decision\""));
        assert!(resp.contains("\"decision\":\"deny\""));
        assert!(daemon
            .get_agent(AgentKind::Claude, "claude-x")
            .unwrap()
            .resolved_permissions
            .is_empty());
    }

    #[test]
    fn session_abort_removes_agent_so_heartbeat_stops() {
        let mut daemon = BridgeDaemon::new();
        register_claude(&mut daemon);
        daemon
            .handle_json_line(
                r#"{"type":"permission.request","permission":{"agentId":"claude-x","kind":"claude","reqId":1,"tool":"Bash","argsSummary":"rm -rf /"}}"#,
            )
            .unwrap();
        daemon
            .handle_json_line(
                r#"{"type":"session.abort","abort":{"agentId":"claude-x","kind":"claude"}}"#,
            )
            .unwrap();
        // After abort the agent is dropped from `registered` — the periodic
        // SessionHeartbeat thread iterates `registered.values()`, so dropping
        // here is what makes the board side observe missed heartbeats and
        // gray-then-drop the SID.
        assert!(daemon.get_agent(AgentKind::Claude, "claude-x").is_none());
    }

    #[test]
    fn hex_round_trip_handles_empty_and_data() {
        assert_eq!(hex_encode(&[]), "");
        assert_eq!(hex_decode("").unwrap(), Vec::<u8>::new());
        let bytes = b"\x1b[2J\x1b[Hhello\r\n";
        let encoded = hex_encode(bytes);
        assert_eq!(hex_decode(&encoded).unwrap(), bytes.to_vec());
        assert_eq!(hex_decode("0a0B").unwrap(), vec![10, 11]);
        assert!(hex_decode("0").is_none());
        assert!(hex_decode("zz").is_none());
    }

    #[test]
    fn terminal_stream_buffers_until_session_focus_then_replays() {
        let mut daemon = BridgeDaemon::new();
        daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"claude-launch-1","kind":"claude","name":"AIKB","cwd":"/work","fromLaunch":true}}"#,
            )
            .unwrap();
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 5,
                payload: vec![SessionStatusByte::Ok as u8],
            })
            .unwrap();
        let _ = daemon.drain_board_outbox();

        let payload = b"\x1b[31mhello\x1b[0m";
        let hex = hex_encode(payload);
        let envelope = format!(
            r#"{{"type":"terminal.stream","stream":{{"agentId":"claude-launch-1","kind":"claude","dataHex":"{hex}"}}}}"#
        );
        let resp = daemon.handle_json_line(&envelope).unwrap();
        assert!(resp.contains("terminal.stream.accepted"));

        let outbox = daemon.drain_board_outbox();
        assert!(
            outbox.is_empty(),
            "unfocused terminal bytes must be cached, not pushed to the board"
        );

        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionFocus,
                sid: 5,
                payload: Vec::new(),
            })
            .unwrap();

        let outbox = daemon.drain_board_outbox();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].cmd, Cmd::Vt100Stream);
        assert_eq!(outbox[0].sid, 5);
        assert!(outbox[0].payload.starts_with(SCREEN_CLEAR));
        assert!(outbox[0].payload.ends_with(payload));
    }

    #[test]
    fn terminal_stream_pushes_live_bytes_when_focused() {
        let mut daemon = BridgeDaemon::new();
        daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"claude-launch-1","kind":"claude","name":"AIKB","cwd":"/work","fromLaunch":true}}"#,
            )
            .unwrap();
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 5,
                payload: vec![SessionStatusByte::Ok as u8],
            })
            .unwrap();
        let _ = daemon.drain_board_outbox();
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionFocus,
                sid: 5,
                payload: Vec::new(),
            })
            .unwrap();
        let _ = daemon.drain_board_outbox();

        let payload = b"\x1b[31mhello\x1b[0m";
        let hex = hex_encode(payload);
        let envelope = format!(
            r#"{{"type":"terminal.stream","stream":{{"agentId":"claude-launch-1","kind":"claude","dataHex":"{hex}"}}}}"#
        );
        let resp = daemon.handle_json_line(&envelope).unwrap();
        assert!(resp.contains("terminal.stream.accepted"));

        let outbox = daemon.drain_board_outbox();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].cmd, Cmd::Vt100Stream);
        assert_eq!(outbox[0].sid, 5);
        assert_eq!(outbox[0].payload, payload.to_vec());
    }

    #[test]
    fn terminal_stream_mirrors_to_attached_agent() {
        let mut daemon = BridgeDaemon::new();
        daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"terminal-1","kind":"terminal","name":"PowerShell","cwd":"/work","fromLaunch":true}}"#,
            )
            .unwrap();
        let _ = daemon.drain_board_outbox();

        daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"codex-1","kind":"codex","name":"codex","cwd":"/work","fromLaunch":true,"parentKind":"terminal","parentAgentId":"terminal-1"}}"#,
            )
            .unwrap();
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 6,
                payload: vec![SessionStatusByte::Ok as u8],
            })
            .unwrap();
        let _ = daemon.drain_board_outbox();

        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionFocus,
                sid: 6,
                payload: Vec::new(),
            })
            .unwrap();
        let _ = daemon.drain_board_outbox();

        let payload = b"codex tui bytes";
        let hex = hex_encode(payload);
        let envelope = format!(
            r#"{{"type":"terminal.stream","stream":{{"agentId":"terminal-1","kind":"terminal","dataHex":"{hex}"}}}}"#
        );
        let resp = daemon.handle_json_line(&envelope).unwrap();
        assert!(resp.contains("terminal.stream.accepted"));

        let outbox = daemon.drain_board_outbox();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].cmd, Cmd::Vt100Stream);
        assert_eq!(outbox[0].sid, 6);
        assert_eq!(outbox[0].payload, payload.to_vec());
    }

    #[test]
    fn attached_agent_focus_replays_mirrored_screen_after_registration() {
        let mut daemon = BridgeDaemon::new();
        daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"terminal-1","kind":"terminal","name":"PowerShell","cwd":"/work","fromLaunch":true}}"#,
            )
            .unwrap();
        assert!(daemon.drain_board_outbox().is_empty());

        daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"codex-1","kind":"codex","name":"codex","cwd":"/work","fromLaunch":true,"parentKind":"terminal","parentAgentId":"terminal-1"}}"#,
            )
            .unwrap();
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 6,
                payload: vec![SessionStatusByte::Ok as u8],
            })
            .unwrap();
        let _ = daemon.drain_board_outbox();

        let payload = b"codex tui first frame";
        let hex = hex_encode(payload);
        let envelope = format!(
            r#"{{"type":"terminal.stream","stream":{{"agentId":"terminal-1","kind":"terminal","dataHex":"{hex}"}}}}"#
        );
        let resp = daemon.handle_json_line(&envelope).unwrap();
        assert!(resp.contains("terminal.stream.accepted"));
        assert!(daemon.drain_board_outbox().is_empty());

        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionFocus,
                sid: 6,
                payload: Vec::new(),
            })
            .unwrap();
        let outbox = daemon.drain_board_outbox();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].cmd, Cmd::Vt100Stream);
        assert_eq!(outbox[0].sid, 6);
        assert!(outbox[0].payload.starts_with(SCREEN_CLEAR));
        assert!(outbox[0].payload.ends_with(payload));
    }

    #[test]
    fn attached_agent_focus_does_not_replay_raw_terminal_history() {
        let mut daemon = BridgeDaemon::new();
        daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"terminal-1","kind":"terminal","name":"PowerShell","cwd":"/work","fromLaunch":true}}"#,
            )
            .unwrap();
        assert!(daemon.drain_board_outbox().is_empty());

        let payload = b"existing codex screen";
        let hex = hex_encode(payload);
        let envelope = format!(
            r#"{{"type":"terminal.stream","stream":{{"agentId":"terminal-1","kind":"terminal","dataHex":"{hex}"}}}}"#
        );
        daemon.handle_json_line(&envelope).unwrap();
        let _ = daemon.drain_board_outbox();

        daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"codex-1","kind":"codex","name":"codex","cwd":"/work","fromLaunch":true,"parentKind":"terminal","parentAgentId":"terminal-1"}}"#,
            )
            .unwrap();
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 6,
                payload: vec![SessionStatusByte::Ok as u8],
            })
            .unwrap();
        let _ = daemon.drain_board_outbox();

        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionFocus,
                sid: 6,
                payload: Vec::new(),
            })
            .unwrap();
        let outbox = daemon.drain_board_outbox();
        assert!(
            outbox.is_empty(),
            "attached agent focus must not replay raw TUI history"
        );
    }

    #[test]
    fn attached_agent_focus_does_not_clear_when_agent_buffer_empty() {
        let mut daemon = BridgeDaemon::new();
        daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"terminal-1","kind":"terminal","name":"PowerShell","cwd":"/work","fromLaunch":true}}"#,
            )
            .unwrap();
        assert!(daemon.drain_board_outbox().is_empty());

        daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"codex-1","kind":"codex","name":"codex","cwd":"/work","fromLaunch":true,"parentKind":"terminal","parentAgentId":"terminal-1"}}"#,
            )
            .unwrap();
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 6,
                payload: vec![SessionStatusByte::Ok as u8],
            })
            .unwrap();
        let _ = daemon.drain_board_outbox();

        let Some(agent) = daemon.get_agent(AgentKind::Codex, "codex-1") else {
            panic!("codex agent not registered");
        };
        assert!(agent.terminal_snapshot().is_empty());

        let payload = b"parent screen after codex registration";
        daemon
            .registered
            .get_mut(&(AgentKind::Terminal, "terminal-1".to_string()))
            .unwrap()
            .append_terminal_bytes(payload);

        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionFocus,
                sid: 6,
                payload: Vec::new(),
            })
            .unwrap();
        let outbox = daemon.drain_board_outbox();
        assert!(
            outbox.is_empty(),
            "attached agent focus must not clear the existing terminal view"
        );
    }

    #[test]
    fn terminal_stream_for_unknown_agent_errors() {
        let mut daemon = BridgeDaemon::new();
        let envelope = format!(
            r#"{{"type":"terminal.stream","stream":{{"agentId":"missing","kind":"claude","dataHex":"{}"}}}}"#,
            hex_encode(b"x"),
        );
        assert!(daemon.handle_json_line(&envelope).is_err());
    }

    #[test]
    fn token_update_for_unknown_agent_errors() {
        let mut daemon = BridgeDaemon::new();
        let resp = daemon.handle_json_line(
            r#"{"type":"token.update","token":{"agentId":"ghost","kind":"claude","input":1,"output":2,"costCents":0}}"#,
        );
        assert!(resp.is_err());
    }

    fn discovered_session(agent_id: &str, name: &str, cwd: &str) -> AgentSession {
        AgentSession {
            agent_id: agent_id.to_string(),
            kind: AgentKind::Claude,
            name: name.to_string(),
            cwd: cwd.to_string(),
            transcript_path: format!("/tmp/{agent_id}.jsonl"),
            status: AgentStatus::Running,
            terminal_hwnd: None,
        }
    }

    #[test]
    fn register_discovered_sessions_enqueues_request_session() {
        let mut daemon = BridgeDaemon::new();
        let session = discovered_session("claude-001", "AIKB main", "/work/AIKB");
        let added = daemon.register_discovered_sessions(std::slice::from_ref(&session));
        assert_eq!(added, 1);

        let outbox = daemon.drain_board_outbox();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].cmd, Cmd::RequestSession);
        assert_eq!(outbox[0].sid, BoardSid::BROADCAST.raw());
        assert_eq!(outbox[0].payload, b"AIKB main");

        let agent = daemon.get_agent(AgentKind::Claude, "claude-001").unwrap();
        assert_eq!(agent.cwd, "/work/AIKB");
        assert_eq!(agent.status, AgentStatus::Running);
        assert!(agent.board_sid.is_none());
    }

    #[test]
    fn register_discovered_sessions_is_idempotent_when_called_again() {
        let mut daemon = BridgeDaemon::new();
        let session = discovered_session("claude-001", "AIKB main", "/work/AIKB");
        assert_eq!(
            daemon.register_discovered_sessions(std::slice::from_ref(&session)),
            1
        );
        // Second pass with the same session should not double-register or
        // re-enqueue a board session request.
        assert_eq!(
            daemon.register_discovered_sessions(std::slice::from_ref(&session)),
            0
        );
        let outbox = daemon.drain_board_outbox();
        assert_eq!(
            outbox.len(),
            1,
            "expected exactly one REQUEST_SESSION across both passive ticks"
        );
    }

    #[test]
    fn passive_register_then_hook_register_keeps_board_sid_assigned_during_passive() {
        let mut daemon = BridgeDaemon::new();
        let session = discovered_session("claude-001", "from-passive", "/work/AIKB");
        daemon.register_discovered_sessions(std::slice::from_ref(&session));

        // Board responds to passive's REQUEST_SESSION with sid=7.
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 7,
                payload: vec![SessionStatusByte::Ok as u8],
            })
            .unwrap();
        let _ = daemon.drain_board_outbox();

        // Hook later registers the same agent with a different display name.
        let resp = daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"claude-001","kind":"claude","name":"from-hook","cwd":"/work/AIKB"}}"#,
            )
            .unwrap();
        assert!(resp.contains("agent.registered"));

        let agent = daemon.get_agent(AgentKind::Claude, "claude-001").unwrap();
        assert_eq!(
            agent.board_sid,
            Some(BoardSid::new(7)),
            "hook must reuse board_sid that passive already won"
        );
        assert_eq!(agent.name, "from-hook");
    }

    #[test]
    fn unbound_passive_session_is_not_pruned_while_request_may_be_queued() {
        let mut daemon = BridgeDaemon::new();
        let session = discovered_session("claude-001", "AIKB", "/work/AIKB");
        assert_eq!(daemon.register_discovered_sessions(&[session]), 1);

        for _ in 0..(PASSIVE_MISSING_GRACE_POLLS + 2) {
            let dropped = daemon.prune_missing_passive_sessions(&[]);
            assert!(dropped.is_empty());
        }

        assert!(daemon.get_agent(AgentKind::Claude, "claude-001").is_some());
        assert_eq!(
            daemon.drain_board_outbox().len(),
            1,
            "must not enqueue duplicate REQUEST_SESSION while HID is offline"
        );
    }

    #[test]
    fn hook_registered_agent_is_not_pruned_by_passive_snapshot_miss() {
        let mut daemon = BridgeDaemon::new();
        daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"claude-001","kind":"claude","name":"hook","cwd":"/work/AIKB"}}"#,
            )
            .unwrap();
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 7,
                payload: vec![SessionStatusByte::Ok as u8],
            })
            .unwrap();
        let _ = daemon.drain_board_outbox();

        for _ in 0..(PASSIVE_MISSING_GRACE_POLLS + 2) {
            let dropped = daemon.prune_missing_passive_sessions(&[]);
            assert!(dropped.is_empty());
        }

        let agent = daemon.get_agent(AgentKind::Claude, "claude-001").unwrap();
        assert_eq!(agent.board_sid, Some(BoardSid::new(7)));
        assert!(agent.from_hook);
    }

    fn discovered_turn(agent_id: &str, role: TurnRole, text: &str) -> DiscoveredTurn {
        DiscoveredTurn {
            kind: AgentKind::Claude,
            agent_id: agent_id.to_string(),
            role,
            text: text.to_string(),
        }
    }

    #[test]
    fn ingest_discovered_turns_appends_to_registered_agent() {
        let mut daemon = BridgeDaemon::new();
        let session = discovered_session("claude-001", "AIKB", "/work/AIKB");
        daemon.register_discovered_sessions(std::slice::from_ref(&session));
        let _ = daemon.drain_board_outbox();

        let ingested = daemon.ingest_discovered_turns(&[
            discovered_turn("claude-001", TurnRole::User, "hello board"),
            discovered_turn("claude-001", TurnRole::Assistant, "hi there"),
        ]);
        assert_eq!(ingested, 2);
        let agent = daemon.get_agent(AgentKind::Claude, "claude-001").unwrap();
        assert_eq!(agent.turns.len(), 2);
        assert_eq!(agent.turns[0].role, TurnRole::User);
        assert_eq!(agent.turns[0].text, "hello board");
        assert_eq!(agent.turns[1].role, TurnRole::Assistant);
    }

    #[test]
    fn ingest_discovered_turns_caps_history_at_retention_limit() {
        let mut daemon = BridgeDaemon::new();
        let session = discovered_session("claude-001", "AIKB", "/work/AIKB");
        daemon.register_discovered_sessions(std::slice::from_ref(&session));
        let _ = daemon.drain_board_outbox();

        let cap = max_retained_turns_per_agent();
        let mut turns = Vec::new();
        for i in 0..(cap + 30) {
            turns.push(discovered_turn(
                "claude-001",
                TurnRole::User,
                &format!("turn {i}"),
            ));
        }
        daemon.ingest_discovered_turns(&turns);

        let agent = daemon.get_agent(AgentKind::Claude, "claude-001").unwrap();
        assert_eq!(agent.turns.len(), cap);
        // Oldest 30 should have been dropped; the first retained entry is
        // therefore turn #30.
        assert_eq!(agent.turns[0].text, "turn 30");
        assert_eq!(agent.turns[cap - 1].text, format!("turn {}", cap + 29));
    }

    #[test]
    fn ingest_discovered_turns_drops_unknown_agent() {
        let mut daemon = BridgeDaemon::new();
        let ingested =
            daemon.ingest_discovered_turns(&[discovered_turn("ghost", TurnRole::User, "lost")]);
        assert_eq!(ingested, 0);
        assert!(daemon.drain_board_outbox().is_empty());
    }

    #[test]
    fn ingest_discovered_turns_keeps_history_without_flooding_board() {
        // Passive transcript ingest must NOT push TURN_APPEND to the board —
        // doing so dumps multi-MB jsonl history into the terminal view and
        // breaks the "highly-consistent with the real terminal" requirement.
        // Board terminal fidelity is delivered by start-launched sessions
        // through terminal.stream (Vt100Stream). Passive sessions only
        // appear in the grid; their text is kept in `agent.turns` for
        // future use but never streamed to the board.
        let mut daemon = BridgeDaemon::new();
        let session = discovered_session("claude-001", "AIKB", "/work/AIKB");
        daemon.register_discovered_sessions(std::slice::from_ref(&session));
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 9,
                payload: vec![SessionStatusByte::Ok as u8],
            })
            .unwrap();
        let _ = daemon.drain_board_outbox();

        daemon.ingest_discovered_turns(&[discovered_turn(
            "claude-001",
            TurnRole::User,
            "please run echo",
        )]);
        let outbox = daemon.drain_board_outbox();
        assert!(
            outbox.is_empty(),
            "passive ingest must not push to board, got {} message(s)",
            outbox.len()
        );
        let agent = daemon
            .get_agent(AgentKind::Claude, "claude-001")
            .expect("agent stays registered");
        assert_eq!(agent.turns.len(), 1, "turn cached for in-memory history");
        assert_eq!(agent.turns[0].text, "please run echo");
    }

    #[test]
    fn passive_discovery_then_session_focus_replays_conversation() {
        let mut daemon = BridgeDaemon::new();
        let session = discovered_session("claude-001", "AIKB main", "/work/AIKB");
        daemon.register_discovered_sessions(std::slice::from_ref(&session));

        // Turns arrive before the board has assigned a SID — they must still be
        // stored in agent.turns so the eventual SessionFocus can replay them.
        daemon.ingest_discovered_turns(&[
            discovered_turn("claude-001", TurnRole::User, "hi"),
            discovered_turn("claude-001", TurnRole::Assistant, "hello"),
        ]);

        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionResponse,
                sid: 11,
                payload: vec![SessionStatusByte::Ok as u8],
            })
            .unwrap();
        let _ = daemon.drain_board_outbox();

        // Board user presses CONFIRM on this passive SID → SessionFocus is
        // delivered → daemon must emit a placeholder replay (not turn text).
        // Passive agents have no live VT100 source; per the "highly-consistent
        // with the real terminal" requirement we no longer dump turn history
        // into the terminal view here.
        daemon
            .handle_board_message(HidMessage {
                cmd: Cmd::SessionFocus,
                sid: 11,
                payload: Vec::new(),
            })
            .unwrap();
        let outbox = daemon.drain_board_outbox();
        let replay = outbox
            .iter()
            .find(|msg| msg.cmd == Cmd::Vt100Stream && msg.sid == 11)
            .expect("expected Vt100Stream replay on SessionFocus");
        let text = String::from_utf8_lossy(&replay.payload);
        assert!(
            text.contains("claude"),
            "passive replay must show agent kind in header: {text}"
        );
        assert!(
            text.contains("No live terminal capture"),
            "passive replay must not pretend to be a terminal: {text}"
        );
        assert!(
            !text.contains("hello") && !text.contains("> hi") && !text.contains("* hello"),
            "passive replay must not leak transcript text: {text}"
        );
    }

    #[test]
    fn passive_board_discovery_env_is_opt_in() {
        assert!(!env_flag_enabled(None));
        assert!(!env_flag_enabled(Some("")));
        assert!(!env_flag_enabled(Some("0")));
        assert!(!env_flag_enabled(Some("false")));
        assert!(env_flag_enabled(Some("1")));
        assert!(env_flag_enabled(Some("true")));
        assert!(env_flag_enabled(Some("YES")));
        assert!(env_flag_enabled(Some("on")));
    }

    #[test]
    fn passive_does_not_overwrite_existing_hook_agent() {
        let mut daemon = BridgeDaemon::new();
        daemon
            .handle_json_line(
                r#"{"type":"agent.register","agent":{"agentId":"claude-001","kind":"claude","name":"hook-name","cwd":"/work/AIKB"}}"#,
            )
            .unwrap();
        let _ = daemon.drain_board_outbox();

        // A passive scan finds the same agent with a different name; we should
        // skip it (passive must not clobber state the hook already owns).
        let session = discovered_session("claude-001", "passive-name", "/work/AIKB");
        let added = daemon.register_discovered_sessions(std::slice::from_ref(&session));
        assert_eq!(added, 0);

        let agent = daemon.get_agent(AgentKind::Claude, "claude-001").unwrap();
        assert_eq!(agent.name, "hook-name");
        let outbox = daemon.drain_board_outbox();
        assert!(
            outbox.is_empty(),
            "passive must not enqueue any board traffic for already-known agent"
        );
    }
}
