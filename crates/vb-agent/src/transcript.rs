//! Transcript JSONL parser (M3 填实现)。
//!
//! 支持 Claude Code 和 Codex CLI 的 transcript 格式。每行一个 JSON 事件,
//! 抽出 ConversationTurn + TokenUsage + PermissionRequest。

use vb_core::{ConversationTurn, TokenUsage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEvent {
    Turn(ConversationTurn),
    Usage(TokenUsage),
}

pub trait TranscriptParser {
    fn parse_line(&self, line: &str) -> Option<TranscriptEvent>;
}
