//! 针对新 CMD (M3 起新增) 的 payload 编解码助手。
//!
//! 板端 C 侧 (M4) 会直接按这些字节布局解析。每个 helper 都是单帧 payload,
//! 不超过 HID_MAX_PAYLOAD (58B)。超长字段统一截断 + 末尾保留 1B 标记位。

use vb_core::{BoardSid, TokenUsage};

use crate::{PermissionDecisionByte, TurnRoleByte, HID_MAX_PAYLOAD};

/// TOKEN_USAGE (0x51) payload: 8B input + 8B output + 8B cost_cents = 24B (LE u64)。
pub fn encode_token_usage(usage: TokenUsage) -> Vec<u8> {
    let mut buf = Vec::with_capacity(24);
    buf.extend_from_slice(&usage.input.to_le_bytes());
    buf.extend_from_slice(&usage.output.to_le_bytes());
    buf.extend_from_slice(&usage.cost_cents.to_le_bytes());
    buf
}

pub fn decode_token_usage(payload: &[u8]) -> Option<TokenUsage> {
    if payload.len() < 24 {
        return None;
    }
    Some(TokenUsage {
        input: u64::from_le_bytes(payload[0..8].try_into().ok()?),
        output: u64::from_le_bytes(payload[8..16].try_into().ok()?),
        cost_cents: u64::from_le_bytes(payload[16..24].try_into().ok()?),
    })
}

/// TURN_APPEND (0x52) payload: role(1B) + text_chunk(≤57B UTF-8)。
/// 长 turn 调用方分多帧, 板端按 sid+role 追加同一个 buffer。
pub fn encode_turn_append(role: TurnRoleByte, text: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(role as u8);
    let max_text = HID_MAX_PAYLOAD - 1;
    let truncated = if text.len() > max_text {
        // 安全截断 UTF-8: 找最后一个完整字符边界
        let mut cut = max_text;
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        &text[..cut]
    } else {
        text
    };
    buf.extend_from_slice(truncated.as_bytes());
    buf
}

pub fn decode_turn_append(payload: &[u8]) -> Option<(TurnRoleByte, &str)> {
    if payload.is_empty() {
        return None;
    }
    let role = match payload[0] {
        0 => TurnRoleByte::User,
        1 => TurnRoleByte::Assistant,
        2 => TurnRoleByte::Tool,
        3 => TurnRoleByte::System,
        _ => return None,
    };
    let text = std::str::from_utf8(&payload[1..]).ok()?;
    Some((role, text))
}

/// PERMISSION_REQ (0x53) payload: req_id(8B LE u64) + tool_len(1B) + tool + args_summary。
/// tool 最多 24 字节, args_summary 占剩余。
pub fn encode_permission_request(req_id: u64, tool: &str, args_summary: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&req_id.to_le_bytes());
    let tool_len = tool.len().min(24);
    buf.push(tool_len as u8);
    buf.extend_from_slice(&tool.as_bytes()[..tool_len]);
    let remaining = HID_MAX_PAYLOAD.saturating_sub(buf.len());
    let args_len = args_summary.len().min(remaining);
    buf.extend_from_slice(&args_summary.as_bytes()[..args_len]);
    buf
}

pub fn decode_permission_request(payload: &[u8]) -> Option<(u64, &str, &str)> {
    if payload.len() < 9 {
        return None;
    }
    let req_id = u64::from_le_bytes(payload[0..8].try_into().ok()?);
    let tool_len = payload[8] as usize;
    if 9 + tool_len > payload.len() {
        return None;
    }
    let tool = std::str::from_utf8(&payload[9..9 + tool_len]).ok()?;
    let args = std::str::from_utf8(&payload[9 + tool_len..]).ok()?;
    Some((req_id, tool, args))
}

/// PERMISSION_RES (0x12, B→H) payload: req_id(8B) + decision(1B)。
pub fn encode_permission_response(req_id: u64, decision: PermissionDecisionByte) -> Vec<u8> {
    let mut buf = Vec::with_capacity(9);
    buf.extend_from_slice(&req_id.to_le_bytes());
    buf.push(decision as u8);
    buf
}

pub fn decode_permission_response(payload: &[u8]) -> Option<(u64, PermissionDecisionByte)> {
    if payload.len() < 9 {
        return None;
    }
    let req_id = u64::from_le_bytes(payload[0..8].try_into().ok()?);
    let decision = match payload[8] {
        0 => PermissionDecisionByte::Allow,
        1 => PermissionDecisionByte::Deny,
        2 => PermissionDecisionByte::Always,
        _ => return None,
    };
    Some((req_id, decision))
}

/// ABORT_SESSION (0x13, B→H) payload: 仅 sid 即可, 但 header 已带 sid,
/// payload 留空作为 sentinel。
pub fn encode_abort_session() -> Vec<u8> {
    Vec::new()
}

/// AGENT_META (0x54) payload: kind(1B) + cwd_len(1B) + cwd + branch。
/// kind: 0=Claude 1=Codex 2=VsCode 3=Cursor 4=Browser 0xFF=Unknown
pub fn encode_agent_meta(kind: u8, cwd: &str, branch: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(kind);
    let cwd_max = HID_MAX_PAYLOAD.saturating_sub(2 + branch.len().min(16));
    let cwd_len = cwd.len().min(cwd_max).min(255);
    buf.push(cwd_len as u8);
    buf.extend_from_slice(&cwd.as_bytes()[..cwd_len]);
    let remaining = HID_MAX_PAYLOAD.saturating_sub(buf.len());
    let branch_len = branch.len().min(remaining);
    buf.extend_from_slice(&branch.as_bytes()[..branch_len]);
    buf
}

pub fn decode_agent_meta(payload: &[u8]) -> Option<(u8, &str, &str)> {
    if payload.len() < 2 {
        return None;
    }
    let kind = payload[0];
    let cwd_len = payload[1] as usize;
    if 2 + cwd_len > payload.len() {
        return None;
    }
    let cwd = std::str::from_utf8(&payload[2..2 + cwd_len]).ok()?;
    let branch = std::str::from_utf8(&payload[2 + cwd_len..]).ok()?;
    Some((kind, cwd, branch))
}

/// 用于 vb-host CLI 调试时显示 sid 友好格式。
pub fn fmt_sid(sid: BoardSid) -> String {
    if sid.is_broadcast() {
        "BROADCAST".to_string()
    } else {
        format!("sid={}", sid.raw())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_usage_roundtrip() {
        let u = TokenUsage {
            input: 12_345,
            output: 67_890,
            cost_cents: 47,
        };
        assert_eq!(decode_token_usage(&encode_token_usage(u)), Some(u));
    }

    #[test]
    fn turn_append_roundtrip_short() {
        let bytes = encode_turn_append(TurnRoleByte::Assistant, "hi there");
        let (role, text) = decode_turn_append(&bytes).unwrap();
        assert_eq!(role, TurnRoleByte::Assistant);
        assert_eq!(text, "hi there");
    }

    #[test]
    fn turn_append_truncates_long_utf8_safely() {
        let long = "a".repeat(100);
        let bytes = encode_turn_append(TurnRoleByte::User, &long);
        assert!(bytes.len() <= HID_MAX_PAYLOAD);
        let (role, text) = decode_turn_append(&bytes).unwrap();
        assert_eq!(role, TurnRoleByte::User);
        assert_eq!(text.len(), HID_MAX_PAYLOAD - 1);
    }

    #[test]
    fn turn_append_utf8_boundary() {
        // 三字节 UTF-8 (中文) — 截断不能砍到字符中间
        let cn = "中".repeat(30);
        let bytes = encode_turn_append(TurnRoleByte::Assistant, &cn);
        let (_role, text) = decode_turn_append(&bytes).unwrap();
        assert!(text.chars().count() <= cn.chars().count());
        assert!(text.chars().all(|c| c == '中'));
    }

    #[test]
    fn permission_request_roundtrip() {
        let bytes = encode_permission_request(0xDEAD_BEEF_CAFE_BABE, "Write", "main.rs:42");
        let (req_id, tool, args) = decode_permission_request(&bytes).unwrap();
        assert_eq!(req_id, 0xDEAD_BEEF_CAFE_BABE);
        assert_eq!(tool, "Write");
        assert_eq!(args, "main.rs:42");
    }

    #[test]
    fn permission_response_roundtrip() {
        for dec in [
            PermissionDecisionByte::Allow,
            PermissionDecisionByte::Deny,
            PermissionDecisionByte::Always,
        ] {
            let bytes = encode_permission_response(42, dec);
            let (req_id, decoded) = decode_permission_response(&bytes).unwrap();
            assert_eq!(req_id, 42);
            assert_eq!(decoded, dec);
        }
    }

    #[test]
    fn agent_meta_roundtrip() {
        let bytes = encode_agent_meta(1, "/home/rv/AIKB", "main");
        let (kind, cwd, branch) = decode_agent_meta(&bytes).unwrap();
        assert_eq!(kind, 1);
        assert_eq!(cwd, "/home/rv/AIKB");
        assert_eq!(branch, "main");
    }
}
