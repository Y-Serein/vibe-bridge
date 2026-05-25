use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use vb_core::{AgentActivity, AgentActivityKind, AgentKind, AgentSession, AgentStatus, TurnRole};

const CLAUDE_RECENT_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);
const CODEX_RECENT_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

pub fn discover_agent_sessions() -> Result<Vec<AgentSession>, String> {
    let candidates = discover_agent_session_candidates()?;
    Ok(filter_active_sessions(candidates))
}

pub fn discover_agent_session_candidates() -> Result<Vec<AgentSession>, String> {
    let mut sessions = Vec::new();
    for root in default_claude_roots() {
        sessions.extend(discover_claude_sessions_in(&root)?);
    }
    for root in default_codex_roots() {
        sessions.extend(discover_codex_sessions_in(&root)?);
    }
    sessions.sort_by(|a, b| b.agent_id.cmp(&a.agent_id));
    sessions.dedup_by(|a, b| a.kind == b.kind && a.agent_id == b.agent_id);
    Ok(sessions)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentSourceRoots {
    pub homes: Vec<PathBuf>,
    pub claude_roots: Vec<PathBuf>,
    pub codex_roots: Vec<PathBuf>,
}

pub fn agent_source_roots() -> AgentSourceRoots {
    let homes = default_home_dirs();
    let claude_roots = homes
        .iter()
        .map(|home| home.join(".claude").join("projects"))
        .collect();
    let codex_roots = homes
        .iter()
        .map(|home| home.join(".codex").join("sessions"))
        .collect();
    AgentSourceRoots {
        homes,
        claude_roots,
        codex_roots,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentPollSnapshot {
    pub sessions: Vec<AgentSession>,
    pub activities: Vec<AgentActivity>,
    /// Conversation turn text extracted from transcript JSONL. Used by daemon
    /// passive discovery to fill `agent.turns` so the board terminal replay
    /// shows real conversation content without relying on hook events.
    pub turns: Vec<DiscoveredTurn>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredTurn {
    pub kind: AgentKind,
    pub agent_id: String,
    pub role: TurnRole,
    pub text: String,
}

#[derive(Debug, Default)]
pub struct AgentSourcePoller {
    cursors: HashMap<PathBuf, u64>,
    seen_sessions: HashSet<(AgentKind, String)>,
    include_inactive: bool,
}

impl AgentSourcePoller {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_with_inactive(include_inactive: bool) -> Self {
        Self {
            include_inactive,
            ..Self::default()
        }
    }

    /// 不走 active filter 的快速首扫: 跳过 ToolHelp32 进程枚举和 Windows
    /// Terminal UIA 检测, 让 daemon 启动时立刻看到所有 transcript 候选。
    /// 历史 sessions 也会被纳入, 由上层根据真实场景决定是否后续清理。
    pub fn poll_candidates_once(&mut self) -> Result<AgentPollSnapshot, String> {
        self.poll_with_sessions(discover_agent_session_candidates()?)
    }

    pub fn poll_once(&mut self) -> Result<AgentPollSnapshot, String> {
        let sessions = if self.include_inactive {
            discover_agent_session_candidates()?
        } else {
            discover_agent_sessions()?
        };
        self.poll_with_sessions(sessions)
    }

    fn poll_with_sessions(
        &mut self,
        sessions: Vec<AgentSession>,
    ) -> Result<AgentPollSnapshot, String> {
        let mut activities = Vec::new();
        let mut turns = Vec::new();

        for session in &sessions {
            let seen_key = (session.kind, session.agent_id.clone());
            if self.seen_sessions.insert(seen_key) {
                activities.push(AgentActivity {
                    agent_id: session.agent_id.clone(),
                    kind: session.kind,
                    activity: AgentActivityKind::Seen,
                    status: session.status,
                    transcript_path: session.transcript_path.clone(),
                });
            }

            let path = PathBuf::from(&session.transcript_path);
            let Some(appended) = read_appended_lines(&path, &mut self.cursors)? else {
                continue;
            };
            for line in appended.lines() {
                if let Some(activity) =
                    line_to_activity(session.kind, &session.agent_id, &path, line)
                {
                    activities.push(activity);
                }
                if let Some(turn) = line_to_discovered_turn(session.kind, &session.agent_id, line) {
                    turns.push(turn);
                }
            }
        }

        Ok(AgentPollSnapshot {
            sessions,
            activities,
            turns,
        })
    }
}

const MAX_RETAINED_TURNS_PER_AGENT: usize = 50;

/// Cap the rolling turn history kept in memory for each agent. Board terminal
/// replay only renders the most recent few turns, so unbounded growth from a
/// long transcript would just burn RAM.
pub fn max_retained_turns_per_agent() -> usize {
    MAX_RETAINED_TURNS_PER_AGENT
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveAgentProcess {
    pub kind: AgentKind,
    pub cwd: String,
}

fn filter_active_sessions(candidates: Vec<AgentSession>) -> Vec<AgentSession> {
    let counts = active_session_evidence_counts(&candidates);
    filter_sessions_by_active_counts(candidates, counts)
}

fn filter_sessions_by_active_counts(
    mut candidates: Vec<AgentSession>,
    counts: HashMap<(AgentKind, String), usize>,
) -> Vec<AgentSession> {
    candidates.sort_by(|a, b| {
        session_modified_time(b)
            .cmp(&session_modified_time(a))
            .then_with(|| b.transcript_path.cmp(&a.transcript_path))
    });

    // Dedup down to one slot per `(kind, cwd)`. The active scan reports
    // both parent and forked worker processes for codex (`comm=codex` on
    // both), which used to produce 2 slots per logical session and led to
    // two transcript candidates being bound to the same project — board
    // saw duplicate SIDs that thrashed as different jsonl files won the
    // race on each tick. One slot per `(kind, cwd)` keeps the SID stable
    // across ticks; if a user really runs two codex windows in the same
    // directory we display them as one row, which is the right call for
    // the rare case.
    let mut active_slots: Vec<(AgentKind, String)> = counts.into_keys().collect();

    // No active-process evidence at all from any distro — most likely the
    // wsl.exe scan failed entirely. Fall back to recently-modified-and-not-
    // done transcripts so the board isn't empty just because process scan
    // misbehaves. This is the recovery path; normal operation hits the
    // strict branch below.
    if active_slots.is_empty() {
        return fallback_recent_active_candidates(candidates);
    }

    // Strict semantic: a transcript candidate must match a live process. If
    // the process is gone, the session is dropped, regardless of whether the
    // transcript still says "task_started" with no closing event (Typora-
    // closed-abruptly is the canonical bug). status=Running in the jsonl
    // alone is NOT trustworthy — many CLIs never write a `task_complete`
    // when the user just kills the terminal.
    let mut out = Vec::new();
    for session in candidates {
        let mut session = session;
        let sess_cwd = normalize_cwd(&session.cwd);
        let Some(pos) = active_slots.iter().position(|(kind, proc_cwd)| {
            *kind == session.kind && cwd_prefix_matches(&sess_cwd, proc_cwd)
        }) else {
            continue;
        };
        active_slots.swap_remove(pos);
        // Process is verifiably alive: pin status to Running, override any
        // stale Done/Idle/Unknown from the transcript so the board picker
        // doesn't claim the session is dead. WaitingInput is preserved
        // because it's specifically a "alive but blocked" signal.
        if !matches!(session.status, AgentStatus::WaitingInput) {
            session.status = AgentStatus::Running;
        }
        out.push(session);
    }
    sort_and_dedup_sessions(out)
}

/// True if `proc_cwd` (where the agent process was launched) and `sess_cwd`
/// (where the transcript says the agent is working) are on the same branch
/// of the directory tree — i.e., one is a prefix of the other. This handles
/// Claude's behavior of overwriting the transcript `cwd` field whenever the
/// user `cd`s, even though the process itself never moves.
fn cwd_prefix_matches(sess_cwd: &str, proc_cwd: &str) -> bool {
    if sess_cwd == proc_cwd {
        return true;
    }
    let with_sep = |s: &str| {
        if s.ends_with('/') {
            s.to_string()
        } else {
            format!("{s}/")
        }
    };
    let sess = with_sep(sess_cwd);
    let proc = with_sep(proc_cwd);
    sess.starts_with(&proc) || proc.starts_with(&sess)
}

fn fallback_recent_active_candidates(candidates: Vec<AgentSession>) -> Vec<AgentSession> {
    const FALLBACK_WINDOW: Duration = Duration::from_secs(2 * 60 * 60);
    let cutoff = SystemTime::now()
        .checked_sub(FALLBACK_WINDOW)
        .unwrap_or(UNIX_EPOCH);
    let out = candidates
        .into_iter()
        .filter(|session| {
            !matches!(session.status, AgentStatus::Done | AgentStatus::Error)
                && session_modified_time(session) >= cutoff
        })
        .collect();
    sort_and_dedup_sessions(out)
}

fn sort_and_dedup_sessions(mut sessions: Vec<AgentSession>) -> Vec<AgentSession> {
    sessions.sort_by(|a, b| b.agent_id.cmp(&a.agent_id));
    sessions.dedup_by(|a, b| a.kind == b.kind && a.agent_id == b.agent_id);
    sessions
}

fn active_agent_process_counts() -> HashMap<(AgentKind, String), usize> {
    let mut counts = HashMap::new();
    for process in active_agent_processes() {
        *counts
            .entry(active_key(process.kind, &process.cwd))
            .or_insert(0) += 1;
    }
    counts
}

fn active_session_evidence_counts(
    candidates: &[AgentSession],
) -> HashMap<(AgentKind, String), usize> {
    let mut counts = active_agent_process_counts();
    for (key, count) in visible_terminal_session_counts(candidates) {
        counts
            .entry(key)
            .and_modify(|existing| *existing = (*existing).max(count))
            .or_insert(count);
    }
    counts
}

fn visible_terminal_session_counts(
    candidates: &[AgentSession],
) -> HashMap<(AgentKind, String), usize> {
    let Ok(titles) = crate::discover_terminal_titles() else {
        return HashMap::new();
    };
    visible_terminal_session_counts_from_titles(candidates, &titles)
}

fn visible_terminal_session_counts_from_titles(
    candidates: &[AgentSession],
    titles: &[String],
) -> HashMap<(AgentKind, String), usize> {
    let mut labels_by_key: HashMap<(AgentKind, String), Vec<String>> = HashMap::new();
    let mut latest_by_kind: HashMap<AgentKind, Vec<&AgentSession>> = HashMap::new();
    for session in candidates {
        let key = active_key(session.kind, &session.cwd);
        let labels = labels_by_key.entry(key).or_default();
        push_label(labels, &session.name);
        push_label(labels, cwd_basename(&session.cwd));
        latest_by_kind
            .entry(session.kind)
            .or_default()
            .push(session);
    }
    for sessions in latest_by_kind.values_mut() {
        sessions.sort_by(|a, b| {
            session_modified_time(b)
                .cmp(&session_modified_time(a))
                .then_with(|| b.transcript_path.cmp(&a.transcript_path))
        });
    }

    let mut counts = HashMap::new();
    let mut generic_counts: HashMap<AgentKind, usize> = HashMap::new();
    for title in titles {
        let title = normalize_match_text(title);
        if title.is_empty() || looks_like_plain_shell_title(&title) {
            continue;
        }
        if let Some(key) = best_matching_session_key(&title, &labels_by_key) {
            *counts.entry(key).or_insert(0) += 1;
        } else if let Some(kind) = generic_agent_title_kind(&title) {
            *generic_counts.entry(kind).or_insert(0) += 1;
        }
    }
    for (kind, count) in generic_counts {
        if let Some(sessions) = latest_by_kind.get(&kind) {
            for session in sessions.iter().take(count) {
                *counts
                    .entry(active_key(session.kind, &session.cwd))
                    .or_insert(0) += 1;
            }
        }
    }
    counts
}

fn best_matching_session_key(
    title: &str,
    labels_by_key: &HashMap<(AgentKind, String), Vec<String>>,
) -> Option<(AgentKind, String)> {
    let mut best: Option<((AgentKind, String), usize)> = None;
    for (key, labels) in labels_by_key {
        let Some(score) = labels
            .iter()
            .filter(|label| !label.is_empty() && title.contains(label.as_str()))
            .map(String::len)
            .max()
        else {
            continue;
        };
        if best
            .as_ref()
            .map(|(_, current)| score > *current)
            .unwrap_or(true)
        {
            best = Some((key.clone(), score));
        }
    }
    best.map(|(key, _)| key)
}

fn push_label(labels: &mut Vec<String>, label: &str) {
    let label = normalize_match_text(label);
    if label.len() >= 3 && !labels.contains(&label) {
        labels.push(label);
    }
}

fn cwd_basename(cwd: &str) -> &str {
    cwd.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(cwd)
}

fn normalize_match_text(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .replace('\\', "/")
        .replace('_', "-")
}

fn looks_like_plain_shell_title(title: &str) -> bool {
    if matches!(
        title,
        "npm" | "node" | "powershell" | "windows powershell" | "command prompt" | "cmd"
    ) {
        return true;
    }
    if title.contains("windows powershell") || title.contains("command prompt") {
        return true;
    }
    title.contains('@') && (title.contains(": ~") || title.contains(": /") || title.ends_with(":~"))
}

fn generic_agent_title_kind(title: &str) -> Option<AgentKind> {
    if title.contains("claude") {
        Some(AgentKind::Claude)
    } else if title.contains("codex") {
        Some(AgentKind::Codex)
    } else {
        None
    }
}

fn active_key(kind: AgentKind, cwd: &str) -> (AgentKind, String) {
    (kind, normalize_cwd(cwd))
}

fn normalize_cwd(cwd: &str) -> String {
    cwd.trim_end_matches(['/', '\\']).replace('\\', "/")
}

pub fn active_agent_processes() -> Vec<ActiveAgentProcess> {
    if cfg!(windows) {
        active_wsl_agent_processes()
    } else {
        active_unix_agent_processes()
    }
}

fn discover_claude_sessions_in(projects_dir: &Path) -> Result<Vec<AgentSession>, String> {
    let entries = match fs::read_dir(&projects_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("read {}: {err}", projects_dir.display())),
    };

    let cutoff = SystemTime::now()
        .checked_sub(CLAUDE_RECENT_WINDOW)
        .unwrap_or(UNIX_EPOCH);
    let mut sessions = Vec::new();

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let project_dir = entry.path();
        let project_name = entry.file_name().to_string_lossy().to_string();
        let fallback_cwd = resolve_cwd_from_project_hash(&project_name);

        let Ok(files) = fs::read_dir(&project_dir) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            let modified = file
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .unwrap_or(UNIX_EPOCH);
            if modified < cutoff {
                continue;
            }
            let fallback_agent_id = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_default();
            if fallback_agent_id.is_empty() || fallback_agent_id == "subagents" {
                continue;
            }
            let metadata = read_claude_session_metadata(&path);
            let agent_id = metadata
                .agent_id
                .unwrap_or_else(|| fallback_agent_id.clone());
            let cwd = metadata.cwd.unwrap_or_else(|| fallback_cwd.clone());
            let status = metadata.status.unwrap_or(AgentStatus::Unknown);
            sessions.push(AgentSession {
                agent_id: agent_id.clone(),
                kind: AgentKind::Claude,
                name: session_name_from_cwd(&cwd, &agent_id),
                cwd: cwd.clone(),
                transcript_path: path_to_string(&path),
                status,
                terminal_hwnd: None,
            });
        }
    }

    Ok(sessions)
}

fn discover_codex_sessions_in(sessions_root: &Path) -> Result<Vec<AgentSession>, String> {
    let cutoff = SystemTime::now()
        .checked_sub(CODEX_RECENT_WINDOW)
        .unwrap_or(UNIX_EPOCH);
    let mut sessions = Vec::new();

    for path in recent_jsonl_files(sessions_root, cutoff, is_codex_session_file)? {
        let fallback_agent_id = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_default();
        if fallback_agent_id.is_empty() {
            continue;
        }
        let metadata = read_codex_session_metadata(&path);
        let agent_id = metadata.agent_id.unwrap_or(fallback_agent_id);
        let cwd = metadata
            .cwd
            .unwrap_or_else(|| infer_codex_cwd_from_path(&path));
        let status = metadata.status.unwrap_or(AgentStatus::Unknown);
        sessions.push(AgentSession {
            agent_id: agent_id.clone(),
            kind: AgentKind::Codex,
            name: session_name_from_cwd(&cwd, &agent_id),
            cwd,
            transcript_path: path_to_string(&path),
            status,
            terminal_hwnd: None,
        });
    }

    Ok(sessions)
}

fn recent_jsonl_files(
    root: &Path,
    cutoff: SystemTime,
    file_filter: fn(&Path) -> bool,
) -> Result<Vec<PathBuf>, String> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_filter(&path) {
                continue;
            }
            let modified = entry
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .unwrap_or(UNIX_EPOCH);
            if modified >= cutoff {
                out.push(path);
            }
        }
    }

    Ok(out)
}

fn is_codex_session_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with("rollout-"))
            .unwrap_or(false)
}

#[derive(Debug, Default)]
struct SessionMetadata {
    agent_id: Option<String>,
    cwd: Option<String>,
    status: Option<AgentStatus>,
}

fn read_codex_session_metadata(path: &Path) -> SessionMetadata {
    read_jsonl_metadata(path, update_codex_metadata)
}

fn read_claude_session_metadata(path: &Path) -> SessionMetadata {
    read_jsonl_metadata(path, update_claude_metadata)
}

fn read_jsonl_metadata(path: &Path, updater: fn(&mut SessionMetadata, &Value)) -> SessionMetadata {
    let Ok(file) = File::open(path) else {
        return SessionMetadata::default();
    };
    let mut metadata = SessionMetadata::default();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        updater(&mut metadata, &value);
    }
    metadata
}

fn update_codex_metadata(metadata: &mut SessionMetadata, value: &Value) {
    match value.get("type").and_then(Value::as_str) {
        Some("session_meta") => {
            let payload = value.get("payload").unwrap_or(value);
            set_if_text(&mut metadata.agent_id, payload.get("id"));
            set_if_text(&mut metadata.cwd, payload.get("cwd"));
        }
        Some("event_msg") => {
            if let Some(status) = codex_event_status(value.get("payload").unwrap_or(value)) {
                metadata.status = Some(status);
            }
        }
        Some("response_item") => {
            if let Some(status) = codex_response_status(value.get("payload").unwrap_or(value)) {
                metadata.status = Some(status);
            }
        }
        _ => {}
    }
}

fn update_claude_metadata(metadata: &mut SessionMetadata, value: &Value) {
    set_if_text(&mut metadata.agent_id, value.get("sessionId"));
    set_if_text(&mut metadata.cwd, value.get("cwd"));
    if let Some(status) = claude_status(value) {
        metadata.status = Some(status);
    }
}

fn set_if_text(slot: &mut Option<String>, value: Option<&Value>) {
    let Some(text) = value.and_then(Value::as_str).map(str::trim) else {
        return;
    };
    if !text.is_empty() {
        *slot = Some(text.to_string());
    }
}

fn codex_event_status(payload: &Value) -> Option<AgentStatus> {
    let event_type = payload.get("type")?.as_str()?;
    if event_type.contains("approval")
        || event_type.contains("permission")
        || event_type.contains("input")
    {
        return Some(AgentStatus::WaitingInput);
    }

    match event_type {
        "user_message" | "task_started" => Some(AgentStatus::Running),
        "agent_message" => Some(AgentStatus::Idle),
        "task_complete" => Some(AgentStatus::Done),
        "exec_command_end" | "patch_apply_end" => {
            match payload.get("status").and_then(Value::as_str) {
                Some("failed") | Some("error") => Some(AgentStatus::Error),
                _ => Some(AgentStatus::Running),
            }
        }
        _ => None,
    }
}

fn codex_response_status(payload: &Value) -> Option<AgentStatus> {
    match payload.get("type")?.as_str()? {
        "reasoning"
        | "function_call"
        | "function_call_output"
        | "custom_tool_call"
        | "custom_tool_call_output" => Some(AgentStatus::Running),
        "message" => match payload.get("role").and_then(Value::as_str) {
            Some("assistant") => Some(AgentStatus::Idle),
            Some("user") => Some(AgentStatus::Running),
            _ => None,
        },
        _ => None,
    }
}

fn claude_status(value: &Value) -> Option<AgentStatus> {
    match value.get("type")?.as_str()? {
        "user" => Some(AgentStatus::Running),
        "assistant" => {
            if value.get("error").is_some()
                || value.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true)
            {
                return Some(AgentStatus::Error);
            }
            if value
                .get("message")
                .map(message_has_tool_use)
                .unwrap_or(false)
            {
                Some(AgentStatus::Running)
            } else {
                Some(AgentStatus::Idle)
            }
        }
        "progress" => Some(AgentStatus::Running),
        "system" => match value.get("subtype").and_then(Value::as_str) {
            Some("turn_duration") | Some("stop_hook_summary") => Some(AgentStatus::Done),
            _ => None,
        },
        _ => None,
    }
}

fn message_has_tool_use(message: &Value) -> bool {
    message
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("tool_use")
                    || item.get("type").and_then(Value::as_str) == Some("tool_result")
            })
        })
        .unwrap_or(false)
}

fn line_to_activity(
    kind: AgentKind,
    agent_id: &str,
    path: &Path,
    line: &str,
) -> Option<AgentActivity> {
    let value: Value = serde_json::from_str(line).ok()?;
    let (activity, status) = match kind {
        AgentKind::Codex => codex_line_activity_status(&value)?,
        AgentKind::Claude => claude_line_activity_status(&value)?,
        AgentKind::Terminal => return None,
        AgentKind::Unknown => return None,
    };
    Some(AgentActivity {
        agent_id: agent_id.to_string(),
        kind,
        activity,
        status,
        transcript_path: path_to_string(path),
    })
}

fn codex_line_activity_status(value: &Value) -> Option<(AgentActivityKind, AgentStatus)> {
    match value.get("type").and_then(Value::as_str)? {
        "event_msg" => {
            let payload = value.get("payload").unwrap_or(value);
            let status = codex_event_status(payload)?;
            let activity = match status {
                AgentStatus::WaitingInput => AgentActivityKind::WaitingInput,
                AgentStatus::Done => AgentActivityKind::Completed,
                AgentStatus::Error => AgentActivityKind::Error,
                AgentStatus::Idle => AgentActivityKind::AssistantOutput,
                AgentStatus::Running => match payload.get("type").and_then(Value::as_str) {
                    Some("user_message") => AgentActivityKind::UserInput,
                    Some("exec_command_end") | Some("patch_apply_end") => {
                        AgentActivityKind::ToolActivity
                    }
                    _ => AgentActivityKind::ToolActivity,
                },
                AgentStatus::Unknown => return None,
            };
            Some((activity, status))
        }
        "response_item" => {
            let payload = value.get("payload").unwrap_or(value);
            let status = codex_response_status(payload)?;
            let activity = match payload.get("type").and_then(Value::as_str) {
                Some("message") if payload.get("role").and_then(Value::as_str) == Some("user") => {
                    AgentActivityKind::UserInput
                }
                Some("message") => AgentActivityKind::AssistantOutput,
                _ => AgentActivityKind::ToolActivity,
            };
            Some((activity, status))
        }
        _ => None,
    }
}

pub fn line_to_discovered_turn(
    kind: AgentKind,
    agent_id: &str,
    line: &str,
) -> Option<DiscoveredTurn> {
    let value: Value = serde_json::from_str(line).ok()?;
    let (role, text) = match kind {
        AgentKind::Claude => claude_line_to_turn(&value)?,
        AgentKind::Codex => codex_line_to_turn(&value)?,
        AgentKind::Terminal => return None,
        AgentKind::Unknown => return None,
    };
    let text = text.trim();
    if text.is_empty() || is_pseudo_turn_text(text) {
        return None;
    }
    Some(DiscoveredTurn {
        kind,
        agent_id: agent_id.to_string(),
        role,
        text: text.to_string(),
    })
}

/// Drop synthetic placeholder turns that the agent CLIs emit for non-content
/// events (turn aborts, interrupts, system notices). They are not real user
/// or assistant text and must not be surfaced on the board terminal view.
fn is_pseudo_turn_text(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "<turn_aborted>"
            | "<user_interrupt>"
            | "<system>"
            | "<interrupted>"
            | "[turn aborted]"
            | "[user interrupted]"
    ) || lower.starts_with("<system>") && lower.ends_with("</system>")
}

fn claude_line_to_turn(value: &Value) -> Option<(TurnRole, String)> {
    let kind = value.get("type")?.as_str()?;
    let message = value.get("message")?;
    match kind {
        "user" => extract_message_text(message, false).map(|t| (TurnRole::User, t)),
        "assistant" => extract_message_text(message, true).map(|t| (TurnRole::Assistant, t)),
        _ => None,
    }
}

fn extract_message_text(message: &Value, skip_tool_use: bool) -> Option<String> {
    let content = message.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    let items = content.as_array()?;
    let mut buf = String::new();
    for item in items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        if skip_tool_use && (item_type == "tool_use" || item_type == "tool_result") {
            continue;
        }
        if item_type == "tool_result" {
            continue;
        }
        if let Some(text) = item.get("text").and_then(Value::as_str) {
            if !buf.is_empty() {
                buf.push(' ');
            }
            buf.push_str(text);
        }
    }
    if buf.is_empty() {
        None
    } else {
        Some(buf)
    }
}

fn codex_line_to_turn(value: &Value) -> Option<(TurnRole, String)> {
    let outer_type = value.get("type")?.as_str()?;
    match outer_type {
        "event_msg" => {
            let payload = value.get("payload").unwrap_or(value);
            let payload_type = payload.get("type")?.as_str()?;
            let role = match payload_type {
                "user_message" => TurnRole::User,
                "agent_message" => TurnRole::Assistant,
                _ => return None,
            };
            let text = payload.get("message")?.as_str()?.to_string();
            Some((role, text))
        }
        "response_item" => {
            let payload = value.get("payload").unwrap_or(value);
            if payload.get("type")?.as_str()? != "message" {
                return None;
            }
            let role = match payload.get("role").and_then(Value::as_str)? {
                "user" => TurnRole::User,
                "assistant" => TurnRole::Assistant,
                _ => return None,
            };
            let content = payload.get("content")?;
            if let Some(s) = content.as_str() {
                return Some((role, s.to_string()));
            }
            let items = content.as_array()?;
            let mut buf = String::new();
            for item in items {
                let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
                let is_text = matches!(item_type, "input_text" | "output_text" | "text");
                if !is_text {
                    continue;
                }
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    if !buf.is_empty() {
                        buf.push(' ');
                    }
                    buf.push_str(text);
                }
            }
            if buf.is_empty() {
                None
            } else {
                Some((role, buf))
            }
        }
        _ => None,
    }
}

fn claude_line_activity_status(value: &Value) -> Option<(AgentActivityKind, AgentStatus)> {
    let status = claude_status(value)?;
    let activity = match status {
        AgentStatus::Done => AgentActivityKind::Completed,
        AgentStatus::Error => AgentActivityKind::Error,
        AgentStatus::Idle => AgentActivityKind::AssistantOutput,
        AgentStatus::Running => match value.get("type").and_then(Value::as_str) {
            Some("user") => AgentActivityKind::UserInput,
            _ => AgentActivityKind::ToolActivity,
        },
        AgentStatus::WaitingInput => AgentActivityKind::WaitingInput,
        AgentStatus::Unknown => return None,
    };
    Some((activity, status))
}

fn read_appended_lines(
    path: &Path,
    cursors: &mut HashMap<PathBuf, u64>,
) -> Result<Option<String>, String> {
    let len = fs::metadata(path)
        .map_err(|err| format!("stat {}: {err}", path.display()))?
        .len();
    // First time we see a transcript, read it from the beginning so passive
    // discovery captures all already-recorded turns (board terminal replay
    // needs history, not just future appends). Subsequent polls advance the
    // cursor and only read new bytes.
    let offset = cursors.entry(path.to_path_buf()).or_insert(0);
    if len < *offset {
        *offset = len;
        return Ok(None);
    }
    if len == *offset {
        return Ok(None);
    }

    let mut file = File::open(path).map_err(|err| format!("open {}: {err}", path.display()))?;
    file.seek(SeekFrom::Start(*offset))
        .map_err(|err| format!("seek {}: {err}", path.display()))?;
    let mut appended = String::new();
    file.read_to_string(&mut appended)
        .map_err(|err| format!("read {}: {err}", path.display()))?;
    *offset = len;
    Ok(Some(appended))
}

fn session_modified_time(session: &AgentSession) -> SystemTime {
    fs::metadata(&session.transcript_path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(UNIX_EPOCH)
}

fn infer_codex_cwd_from_path(path: &Path) -> String {
    path.parent()
        .and_then(Path::parent)
        .map(path_to_string)
        .unwrap_or_default()
}

fn default_claude_roots() -> Vec<PathBuf> {
    agent_source_roots().claude_roots
}

fn default_codex_roots() -> Vec<PathBuf> {
    agent_source_roots().codex_roots
}

fn default_home_dirs() -> Vec<PathBuf> {
    let mut homes = Vec::new();
    homes.extend(configured_agent_homes());
    if let Some(home) = home_dir() {
        homes.push(home);
    }
    homes.extend(wsl_home_dirs());
    homes.sort();
    homes.dedup();
    homes
}

fn configured_agent_homes() -> Vec<PathBuf> {
    std::env::var_os("VIBE_BRIDGE_AGENT_HOMES")
        .map(|value| {
            value
                .to_string_lossy()
                .split(';')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn wsl_home_dirs() -> Vec<PathBuf> {
    let mut homes = Vec::new();
    if cfg!(windows) {
        for root in [r"\\wsl.localhost", r"\\wsl$"] {
            homes.extend(wsl_home_dirs_from_root(Path::new(root)));
        }
        homes.extend(wsl_home_dirs_from_wsl_exe());
    }
    homes
}

fn wsl_home_dirs_from_wsl_exe() -> Vec<PathBuf> {
    let mut homes = Vec::new();
    for distro in wsl_distro_names() {
        let Some(linux_home) = wsl_linux_home_for_distro(&distro) else {
            continue;
        };
        homes.extend(wsl_unc_homes_for_distro(&distro, &linux_home));
    }
    homes
}

fn active_wsl_agent_processes() -> Vec<ActiveAgentProcess> {
    let debug = std::env::var_os("VIBE_BRIDGE_SCAN_DEBUG").is_some();
    // Base64-encode the scan script and pipe it through `base64 -d | sh` on
    // the Linux side. This bypasses every layer of Windows argv quoting
    // (`Command::new`, CreateProcessW, wsl.exe's own parser) — we discovered
    // that passing the multi-line POSIX script directly via
    // `wsl.exe -d <distro> sh -lc <script>` silently produced an empty stdout
    // even though the same script ran fine inside the WSL shell. The
    // failure mode is: the wsl.exe arg parser collapsed/escaped the embedded
    // quotes and the script reaching bash was effectively a no-op.
    let encoded = base64_encode_bytes(ACTIVE_AGENT_PROCESS_SCRIPT.as_bytes());
    let wrapper = format!("echo {encoded} | base64 -d | sh");

    let mut processes = Vec::new();
    for distro in wsl_distro_names() {
        let output = Command::new("wsl.exe")
            .args(["-d", &distro, "--", "sh", "-lc", &wrapper])
            .output();
        let output = match output {
            Ok(o) => o,
            Err(err) => {
                if debug {
                    eprintln!("[scan] wsl.exe -d {distro}: spawn failed: {err}");
                }
                continue;
            }
        };
        if !output.status.success() {
            if debug {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!(
                    "[scan] wsl.exe -d {distro}: exit {:?} stderr={stderr}",
                    output.status.code()
                );
            }
            continue;
        }
        let parsed = parse_active_agent_process_output(&output.stdout);
        if debug {
            let preview: String = String::from_utf8_lossy(&output.stdout)
                .chars()
                .take(160)
                .collect();
            eprintln!(
                "[scan] {distro}: {} agent process(es) [stdout {} bytes]: {:?}",
                parsed.len(),
                output.stdout.len(),
                parsed
                    .iter()
                    .map(|p| format!("{}:{}", p.kind.as_str(), p.cwd))
                    .collect::<Vec<_>>()
            );
            if !output.stdout.is_empty() {
                eprintln!("[scan]   stdout preview: {preview:?}");
            }
        }
        processes.extend(parsed);
    }
    processes
}

/// Tiny std-only base64 encoder; we don't want a new dep just to pipe a
/// scan script through wsl.exe. RFC 4648 alphabet, padded.
fn base64_encode_bytes(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(ALPHABET[(b0 >> 2) as usize] as char);
        out.push(ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(b2 & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn active_unix_agent_processes() -> Vec<ActiveAgentProcess> {
    let Ok(output) = Command::new("sh")
        .args(["-lc", ACTIVE_AGENT_PROCESS_SCRIPT])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_active_agent_process_output(&output.stdout)
}

const ACTIVE_AGENT_PROCESS_SCRIPT: &str = r#"
for p in /proc/[0-9]*; do
  [ -r "$p/comm" ] || continue
  [ -r "$p/cmdline" ] || continue
  comm=$(cat "$p/comm" 2>/dev/null) || continue
  cmd=$(tr '\0' ' ' < "$p/cmdline" 2>/dev/null)
  exe=$(tr '\0' '\n' < "$p/cmdline" 2>/dev/null | sed -n '1p')
  base=${exe##*/}
  kind=
  case "$comm" in
    codex*) kind=codex ;;
    claude*) kind=claude ;;
    node|nodejs|npm|npx|bun)
      case "$cmd" in
        */codex*|*codex-cli*) kind=codex ;;
        */claude*|*claude-code*) kind=claude ;;
        *) continue ;;
      esac
      ;;
    *)
      case "$base" in
        codex*) kind=codex ;;
        claude*) kind=claude ;;
        *) continue ;;
      esac
      ;;
  esac
  cwd=$(readlink "$p/cwd" 2>/dev/null) || continue
  printf '%s\t%s\n' "$kind" "$cwd"
done
"#;

fn parse_active_agent_process_output(bytes: &[u8]) -> Vec<ActiveAgentProcess> {
    decode_wsl_output(bytes)
        .lines()
        .filter_map(|line| {
            let (kind, cwd) = line.split_once('\t')?;
            let kind = AgentKind::from_label(kind);
            if kind == AgentKind::Unknown || cwd.trim().is_empty() {
                return None;
            }
            Some(ActiveAgentProcess {
                kind,
                cwd: cwd.trim().to_string(),
            })
        })
        .collect()
}

fn wsl_distro_names() -> Vec<String> {
    let Ok(output) = Command::new("wsl.exe").args(["--list", "--quiet"]).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    wsl_distro_names_from_output(&output.stdout)
}

fn wsl_linux_home_for_distro(distro: &str) -> Option<String> {
    let output = Command::new("wsl.exe")
        .args(["-d", distro, "sh", "-lc", "printf %s \"$HOME\""])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let home = decode_wsl_output(&output.stdout).trim().to_string();
    if home.starts_with('/') {
        Some(home)
    } else {
        None
    }
}

fn wsl_unc_homes_for_distro(distro: &str, linux_home: &str) -> Vec<PathBuf> {
    let relative_home = linux_home.trim_start_matches('/').replace('/', r"\");
    [r"\\wsl.localhost", r"\\wsl$"]
        .into_iter()
        .map(|root| PathBuf::from(format!(r"{root}\{distro}\{relative_home}")))
        .collect()
}

fn wsl_distro_names_from_output(bytes: &[u8]) -> Vec<String> {
    decode_wsl_output(bytes)
        .lines()
        .map(|line| line.trim().trim_matches('\u{feff}').trim_matches('\0'))
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn decode_wsl_output(bytes: &[u8]) -> String {
    let likely_utf16 = bytes.len() >= 2 && bytes.iter().skip(1).step_by(2).any(|byte| *byte == 0);
    let decoded = if likely_utf16 {
        let words = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&words)
    } else {
        String::from_utf8_lossy(bytes).to_string()
    };
    decoded.replace('\0', "")
}

fn wsl_home_dirs_from_root(root: &Path) -> Vec<PathBuf> {
    let mut homes = Vec::new();
    let Ok(distros) = fs::read_dir(root) else {
        return homes;
    };
    for distro in distros.flatten() {
        let home_root = distro.path().join("home");
        let Ok(users) = fs::read_dir(&home_root) else {
            continue;
        };
        for user in users.flatten() {
            if user.file_type().map(|ty| ty.is_dir()).unwrap_or(false) {
                homes.push(user.path());
            }
        }
    }
    homes
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn session_name_from_cwd(cwd: &str, fallback_id: &str) -> String {
    let trimmed = cwd.trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            let end = fallback_id.len().min(8);
            format!("S-{}", &fallback_id[..end])
        })
}

fn resolve_cwd_from_project_hash(hash: &str) -> String {
    let trimmed = hash.trim_start_matches('-');
    if trimmed.is_empty() {
        return String::new();
    }
    if cfg!(windows) {
        resolve_windows_cwd_hash(trimmed)
    } else {
        format!("/{}", trimmed.replace('-', "/"))
    }
}

fn resolve_windows_cwd_hash(trimmed: &str) -> String {
    let parts: Vec<&str> = trimmed.split('-').filter(|part| !part.is_empty()).collect();
    if parts.len() >= 2 && parts[0].len() == 1 {
        let mut out = format!("{}:\\", parts[0].to_ascii_uppercase());
        out.push_str(&parts[1..].join("\\"));
        out
    } else {
        trimmed.replace('-', "\\")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn resolves_unix_project_hash_to_cwd() {
        if cfg!(windows) {
            return;
        }
        assert_eq!(
            resolve_cwd_from_project_hash("-home-rv_nano-AIKB"),
            "/home/rv_nano/AIKB"
        );
    }

    #[test]
    fn session_name_uses_last_path_segment() {
        assert_eq!(
            session_name_from_cwd("/home/rv_nano/AIKB", "abcdef"),
            "AIKB"
        );
        assert_eq!(session_name_from_cwd("", "abcdef123"), "S-abcdef12");
    }

    #[test]
    fn codex_filter_requires_rollout_jsonl() {
        assert!(is_codex_session_file(Path::new("rollout-2026-05-21.jsonl")));
        assert!(!is_codex_session_file(Path::new("history.jsonl")));
        assert!(!is_codex_session_file(Path::new("rollout-2026-05-21.txt")));
    }

    #[test]
    fn codex_metadata_uses_session_meta_cwd_and_id() {
        let root = make_temp_dir("codex-meta");
        let session_dir = root.join("2026").join("05").join("21");
        fs::create_dir_all(&session_dir).unwrap();
        let path = session_dir.join("rollout-2026-05-21T00-00-00-abc.jsonl");
        write_lines(
            &path,
            &[
                r#"{"type":"session_meta","payload":{"id":"codex-session-1","cwd":"/home/rv_nano/project","originator":"codex-tui","source":"cli"}}"#,
                r#"{"type":"event_msg","payload":{"type":"task_started"}}"#,
                r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#,
            ],
        );

        let sessions = discover_codex_sessions_in(&root).unwrap();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].agent_id, "codex-session-1");
        assert_eq!(sessions[0].cwd, "/home/rv_nano/project");
        assert_eq!(sessions[0].name, "project");
        assert_eq!(sessions[0].status, AgentStatus::Done);
    }

    #[test]
    fn claude_metadata_uses_jsonl_cwd_and_session_id() {
        let root = make_temp_dir("claude-meta");
        let project_dir = root.join("-wrong-path");
        fs::create_dir_all(&project_dir).unwrap();
        let path = project_dir.join("fallback-id.jsonl");
        write_lines(
            &path,
            &[
                r#"{"type":"user","sessionId":"claude-session-1","cwd":"/home/rv_nano/AIKB"}"#,
                r#"{"type":"assistant","sessionId":"claude-session-1","cwd":"/home/rv_nano/AIKB","message":{"content":[{"type":"tool_use"}]}}"#,
            ],
        );

        let sessions = discover_claude_sessions_in(&root).unwrap();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].agent_id, "claude-session-1");
        assert_eq!(sessions[0].cwd, "/home/rv_nano/AIKB");
        assert_eq!(sessions[0].status, AgentStatus::Running);
    }

    #[test]
    fn activity_event_does_not_keep_message_text() {
        let path = Path::new("/tmp/rollout-demo.jsonl");
        let activity = line_to_activity(
            AgentKind::Codex,
            "codex-session-2",
            path,
            r#"{"type":"event_msg","payload":{"type":"agent_message","message":"private answer text"}}"#,
        )
        .unwrap();

        assert_eq!(activity.activity, AgentActivityKind::AssistantOutput);
        assert_eq!(activity.status, AgentStatus::Idle);
        assert_eq!(activity.agent_id, "codex-session-2");
        assert_eq!(activity.transcript_path, "/tmp/rollout-demo.jsonl");
    }

    #[test]
    fn parses_utf16_wsl_distro_output() {
        let bytes = "Ubuntu-22.04\r\nDebian\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();

        assert_eq!(
            wsl_distro_names_from_output(&bytes),
            vec!["Ubuntu-22.04".to_string(), "Debian".to_string()]
        );
    }

    #[test]
    fn builds_wsl_unc_homes_from_distro_and_linux_home() {
        let homes = wsl_unc_homes_for_distro("Ubuntu-22.04", "/home/serein");
        let rendered = homes
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        assert!(rendered.contains(&r"\\wsl.localhost\Ubuntu-22.04\home\serein".to_string()));
        assert!(rendered.contains(&r"\\wsl$\Ubuntu-22.04\home\serein".to_string()));
    }

    #[test]
    fn active_filter_picks_one_session_per_cwd_even_with_multiple_processes() {
        // Active scan returns the count of processes at each (kind, cwd) —
        // including codex's fork-worker pairs, which used to allocate 2
        // slots per logical session and produced duplicate SIDs. Slot dedup
        // now collapses to 1 per (kind, cwd); only the newest transcript
        // candidate wins, keeping the board SID stable.
        let candidates = vec![
            test_session(
                "old",
                "/home/rv_nano",
                "rollout-2026-05-20T13-30-56-old.jsonl",
            ),
            test_session(
                "new-1",
                "/home/rv_nano",
                "rollout-2026-05-21T11-55-04-new-1.jsonl",
            ),
            test_session(
                "new-2",
                "/home/rv_nano",
                "rollout-2026-05-21T11-38-37-new-2.jsonl",
            ),
            test_session(
                "other",
                "/home/slam/Sipeed/Serein/Typora",
                "rollout-2026-05-21T11-36-43-other.jsonl",
            ),
            test_session(
                "new-3",
                "/home/rv_nano",
                "rollout-2026-05-21T11-32-02-new-3.jsonl",
            ),
        ];
        let mut counts = HashMap::new();
        // Even with 3 process entries (e.g., codex parent + 2 children),
        // we should still only register 1 session for this cwd.
        counts.insert((AgentKind::Codex, "/home/rv_nano".to_string()), 3);

        let filtered = filter_sessions_by_active_counts(candidates, counts);
        let ids = filtered
            .iter()
            .map(|session| session.agent_id.as_str())
            .collect::<Vec<_>>();

        // Only the single newest jsonl wins the (kind, cwd) slot.
        assert_eq!(ids.len(), 1);
        assert!(ids.contains(&"new-1"));
        assert!(!ids.contains(&"new-2"));
        assert!(!ids.contains(&"new-3"));
        assert!(!ids.contains(&"old"));
        assert!(!ids.contains(&"other"));
        let alive = filtered
            .iter()
            .find(|session| session.agent_id == "new-1")
            .unwrap();
        assert_eq!(alive.status, AgentStatus::Running);
    }

    #[test]
    fn visible_terminal_titles_count_open_sessions_by_cwd_name() {
        let candidates = vec![
            test_session(
                "rv-new",
                "/home/rv_nano",
                "rollout-2026-05-21T11-55-04-rv-new.jsonl",
            ),
            test_session(
                "rv-idle",
                "/home/rv_nano",
                "rollout-2026-05-21T11-38-37-rv-idle.jsonl",
            ),
            test_session(
                "typora",
                "/home/slam/Sipeed/Serein/Typora",
                "rollout-2026-05-21T11-36-43-typora.jsonl",
            ),
            test_session(
                "old",
                "/home/slam/Sipeed/NanoUPS",
                "rollout-2026-05-19T15-14-02-old.jsonl",
            ),
        ];
        let titles = vec![
            "rv_nano".to_string(),
            "⠇ rv_nano".to_string(),
            "Typora".to_string(),
        ];

        let counts = visible_terminal_session_counts_from_titles(&candidates, &titles);
        let filtered = filter_sessions_by_active_counts(candidates, counts);
        let ids = filtered
            .iter()
            .map(|session| session.agent_id.as_str())
            .collect::<Vec<_>>();

        // Even though 2 terminal titles match /home/rv_nano (rv-new and
        // rv-idle were both viable candidates), slot dedup picks just one
        // — the most recently modified jsonl. Typora is its own (kind, cwd).
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"rv-new"));
        assert!(ids.contains(&"typora"));
        assert!(!ids.contains(&"rv-idle"));
        assert!(!ids.contains(&"old"));
    }

    #[test]
    fn visible_terminal_titles_ignore_plain_shell_tabs() {
        let candidates = vec![
            test_session(
                "rv-new",
                "/home/rv_nano",
                "rollout-2026-05-21T11-55-04-rv-new.jsonl",
            ),
            test_session(
                "rv-idle",
                "/home/rv_nano",
                "rollout-2026-05-21T11-38-37-rv-idle.jsonl",
            ),
            test_session(
                "rv-third",
                "/home/rv_nano",
                "rollout-2026-05-21T11-32-02-rv-third.jsonl",
            ),
            test_session(
                "typora",
                "/home/slam/Sipeed/Serein/Typora",
                "rollout-2026-05-21T11-36-43-typora.jsonl",
            ),
        ];
        let titles = vec![
            "rv_nano".to_string(),
            "rv_nano@DESKTOP-J9FG6TS: ~".to_string(),
            "管理员: Windows PowerShell".to_string(),
            "npm".to_string(),
            "⠼ Typora".to_string(),
            "⠇ rv_nano".to_string(),
            "管理员: ESP-IDF 5.4".to_string(),
        ];

        let counts = visible_terminal_session_counts_from_titles(&candidates, &titles);
        let filtered = filter_sessions_by_active_counts(candidates, counts);
        let ids = filtered
            .iter()
            .map(|session| session.agent_id.as_str())
            .collect::<Vec<_>>();

        // Plain-shell titles (powershell / npm / ESP-IDF) are still filtered
        // out so they don't poison cwd matching. Slot dedup then collapses
        // the remaining rv_nano titles to a single SID.
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"rv-new"));
        assert!(ids.contains(&"typora"));
        assert!(!ids.contains(&"rv-idle"));
        assert!(!ids.contains(&"rv-third"));
    }

    #[test]
    fn visible_terminal_titles_count_generic_agent_tabs() {
        let candidates = vec![
            test_session(
                "rv-new",
                "/home/rv_nano",
                "rollout-2026-05-21T11-55-04-rv-new.jsonl",
            ),
            test_session(
                "rv-idle",
                "/home/rv_nano",
                "rollout-2026-05-21T11-38-37-rv-idle.jsonl",
            ),
            test_session(
                "typora",
                "/home/slam/Sipeed/Serein/Typora",
                "rollout-2026-05-21T11-36-43-typora.jsonl",
            ),
            test_session_with_kind(
                AgentKind::Claude,
                "claude-code",
                "/home/rv_nano/Sipeed/rv_nano/tools/vibe-bridge",
                "claude-2026-05-21T12-00-00-claude-code.jsonl",
            ),
            test_session(
                "old",
                "/home/slam/Sipeed/NanoUPS",
                "rollout-2026-05-19T15-14-02-old.jsonl",
            ),
        ];
        let titles = vec![
            "npm".to_string(),
            "管理员: Windows PowerShell".to_string(),
            "⠋ Typora".to_string(),
            "管理员: ESP-IDF 5.4".to_string(),
            "✳ Claude Code".to_string(),
            "rv_nano".to_string(),
            "rv_nano@DESKTOP-J9FG6TS: ~".to_string(),
            "管理员: Windows PowerShell".to_string(),
            "管理员: Windows PowerShell".to_string(),
            "⠴ rv_nano".to_string(),
        ];

        let counts = visible_terminal_session_counts_from_titles(&candidates, &titles);
        let filtered = filter_sessions_by_active_counts(candidates, counts);
        let ids = filtered
            .iter()
            .map(|session| session.agent_id.as_str())
            .collect::<Vec<_>>();

        // Two codex titles map to /home/rv_nano (rv-new and rv-idle), the
        // generic "✳ Claude Code" title matches the claude session at the
        // vibe-bridge cwd, and "Typora" matches its codex. After slot dedup
        // we end up with one row per (kind, cwd): codex /home/rv_nano,
        // codex Typora, claude vibe-bridge.
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&"rv-new"));
        assert!(ids.contains(&"typora"));
        assert!(ids.contains(&"claude-code"));
        assert!(!ids.contains(&"rv-idle"));
        assert!(!ids.contains(&"old"));
    }

    fn make_temp_dir(label: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "vb-host-{label}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        path
    }

    fn write_lines(path: &Path, lines: &[&str]) {
        let mut file = File::create(path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    #[test]
    fn claude_user_string_content_becomes_turn() {
        let turn = line_to_discovered_turn(
            AgentKind::Claude,
            "claude-1",
            r#"{"type":"user","message":{"role":"user","content":"hello board"}}"#,
        )
        .unwrap();
        assert_eq!(turn.role, TurnRole::User);
        assert_eq!(turn.text, "hello board");
    }

    #[test]
    fn claude_assistant_array_content_skips_tool_use() {
        let turn = line_to_discovered_turn(
            AgentKind::Claude,
            "claude-1",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"ok"},{"type":"tool_use","name":"Bash","input":{}}]}}"#,
        )
        .unwrap();
        assert_eq!(turn.role, TurnRole::Assistant);
        assert_eq!(turn.text, "ok");
    }

    #[test]
    fn claude_assistant_tool_only_message_is_skipped() {
        assert!(line_to_discovered_turn(
            AgentKind::Claude,
            "claude-1",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#,
        )
        .is_none());
    }

    #[test]
    fn codex_event_msg_user_message_becomes_turn() {
        let turn = line_to_discovered_turn(
            AgentKind::Codex,
            "codex-1",
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"please run tests"}}"#,
        )
        .unwrap();
        assert_eq!(turn.role, TurnRole::User);
        assert_eq!(turn.text, "please run tests");
    }

    #[test]
    fn codex_event_msg_agent_message_becomes_turn() {
        let turn = line_to_discovered_turn(
            AgentKind::Codex,
            "codex-1",
            r#"{"type":"event_msg","payload":{"type":"agent_message","message":"tests pass"}}"#,
        )
        .unwrap();
        assert_eq!(turn.role, TurnRole::Assistant);
        assert_eq!(turn.text, "tests pass");
    }

    #[test]
    fn codex_response_item_message_content_array_becomes_turn() {
        let turn = line_to_discovered_turn(
            AgentKind::Codex,
            "codex-1",
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"},{"type":"function_call"}]}}"#,
        )
        .unwrap();
        assert_eq!(turn.role, TurnRole::Assistant);
        assert_eq!(turn.text, "hello");
    }

    #[test]
    fn non_turn_lines_return_none() {
        assert!(
            line_to_discovered_turn(AgentKind::Claude, "claude-1", r#"{"type":"progress"}"#,)
                .is_none()
        );
        assert!(line_to_discovered_turn(
            AgentKind::Codex,
            "codex-1",
            r#"{"type":"event_msg","payload":{"type":"exec_command_end","status":"ok"}}"#,
        )
        .is_none());
    }

    fn test_session(agent_id: &str, cwd: &str, transcript_path: &str) -> AgentSession {
        test_session_with_kind(AgentKind::Codex, agent_id, cwd, transcript_path)
    }

    fn test_session_with_kind(
        kind: AgentKind,
        agent_id: &str,
        cwd: &str,
        transcript_path: &str,
    ) -> AgentSession {
        AgentSession {
            agent_id: agent_id.to_string(),
            kind,
            name: session_name_from_cwd(cwd, agent_id),
            cwd: cwd.to_string(),
            transcript_path: transcript_path.to_string(),
            status: AgentStatus::Done,
            terminal_hwnd: None,
        }
    }
}
