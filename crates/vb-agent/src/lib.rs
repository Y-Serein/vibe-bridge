//! Agent 抽象层。
//!
//! 每种 agent (Claude Code / Codex CLI / VS Code / Cursor / Browser) 通过 daemon
//! 提供的 LocalIpc 主动向 daemon 注册自身, 持续上报 TokenUsage / ConversationTurn /
//! PermissionRequest, 并接收 PermissionDecision / Abort 回写。
//!
//! M1 只定义 trait 与 transcript parser 模块占位; M3 填 Claude Code 实现, M5 填其他。

use vb_core::{AgentKind, AgentStatus, BoardSid, PermissionDecision};

/// Agent 抽象 — 一个具体 agent 进程/会话的 host 侧视图。
pub trait Agent {
    /// agent 类型 (Claude / Codex / VSCode / Cursor / Browser)。
    fn kind(&self) -> AgentKind;

    /// agent 唯一标识 (推荐: kind + pid + transcript hash)。
    fn agent_id(&self) -> &str;

    /// 当前状态。
    fn status(&self) -> AgentStatus;

    /// 板端分配的 sid (注册成功后填)。
    fn board_sid(&self) -> Option<BoardSid>;

    /// 应用用户的权限决定 (Allow/Deny/Always)。
    fn apply_permission_decision(
        &mut self,
        req_id: u64,
        decision: PermissionDecision,
    ) -> Result<(), AgentError>;

    /// 中止当前 session (Reject 双击 / Abort 命令)。
    fn abort(&mut self) -> Result<(), AgentError>;
}

#[derive(Debug)]
pub enum AgentError {
    NotConnected,
    InvalidRequest(String),
    Io(String),
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConnected => write!(f, "agent not connected"),
            Self::InvalidRequest(msg) => write!(f, "invalid agent request: {msg}"),
            Self::Io(msg) => write!(f, "agent io: {msg}"),
        }
    }
}

impl std::error::Error for AgentError {}

pub mod transcript;
