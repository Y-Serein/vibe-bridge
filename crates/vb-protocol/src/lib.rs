//! AIKB HID 协议常量与编解码。
//!
//! 与板端 `aikb_hid_input.c` / `aikb_lcd_ui.c` 的 CMD 定义保持一一对齐。
//! HID report layout (HOST→BOARD / BOARD→HOST 同结构):
//!
//! ```text
//! offset 0: report_id      (1B)
//! offset 1: command        (1B)
//! offset 2: session_id     (2B, little-endian)
//! offset 4: payload_length (2B, little-endian)
//! offset 6: payload[]      (HID_REPORT_LEN - 6 = 58B 上限)
//! ```
//!
//! 长 payload (VT100 stream / TURN_APPEND 等) 需要分帧, 每帧独立 seq。
//! 编解码细节在 M3 完成; 当前文件只定义常量和命令枚举。

use vb_core::BoardSid;

pub const HID_REPORT_LEN: usize = 64;
pub const HID_HEADER_SIZE: usize = 6;
pub const HID_MAX_PAYLOAD: usize = HID_REPORT_LEN - HID_HEADER_SIZE;

/// Board→host input report id.
pub const REPORT_ID_HOST_BOUND: u8 = 0x10;
/// Host→board output report id.
pub const REPORT_ID_DEVICE_BOUND: u8 = 0x20;
pub const REPORT_ID_FEATURE: u8 = 0x30;

/// 命令字。板端 H = host→board, B = board→host。
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    /// H→B: 插件请求 session id
    RequestSession = 0x01,
    /// B→H: 主系统分配 session id + status byte
    SessionResponse = 0x02,
    /// B→H: session 已失效
    SessionInvalid = 0x03,
    /// H→B: 每 10s/sid 心跳，更新 last_heartbeat
    SessionHeartbeat = 0x04,
    /// B→H: 板端选中 sid (旋钮按下确认)
    SessionFocus = 0x05,

    /// B→H: 物理按键事件 (KEY 0..6, DOWN/UP via bitmap)
    KeyEvent = 0x10,
    /// B→H: 旋钮事件 (delta steps)
    EncoderEvent = 0x11,
    /// B→H: 新增 — 用户对 PermissionRequest 的决定
    PermissionRes = 0x12,
    /// B→H: 新增 — 强制中止 session (SIGINT/Esc 路径)
    AbortSession = 0x13,
    /// B→H: 新增 — 转发文本/键序列到 agent 输入框 (可选)
    SendKey = 0x14,

    /// H→B: VT100 字节流到对应 sid 终端缓冲区
    Vt100Stream = 0x30,
    /// H→B: UI 缩放参数 (字体/行高/列宽)
    UiScaleChange = 0x40,
    /// H→B: 状态广播 (sid + SessionStateByte)
    StatusUpdate = 0x50,

    /// H→B: 新增 — token 用量快照 (input/output/cost)
    TokenUsage = 0x51,
    /// H→B: 新增 — 对话 turn 增量 (role + text chunk)
    TurnAppend = 0x52,
    /// H→B: 新增 — agent 发出待审批请求 (tool + args summary)
    PermissionReq = 0x53,
    /// H→B: 新增 — agent 元信息 (kind + cwd + branch)
    AgentMeta = 0x54,

    /// H→B: 震动/声音/LED 反馈
    FeedbackEvent = 0x60,
    /// 双向: 错误
    Error = 0xFF,
}

impl Cmd {
    pub fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0x01 => Self::RequestSession,
            0x02 => Self::SessionResponse,
            0x03 => Self::SessionInvalid,
            0x04 => Self::SessionHeartbeat,
            0x05 => Self::SessionFocus,
            0x10 => Self::KeyEvent,
            0x11 => Self::EncoderEvent,
            0x12 => Self::PermissionRes,
            0x13 => Self::AbortSession,
            0x14 => Self::SendKey,
            0x30 => Self::Vt100Stream,
            0x40 => Self::UiScaleChange,
            0x50 => Self::StatusUpdate,
            0x51 => Self::TokenUsage,
            0x52 => Self::TurnAppend,
            0x53 => Self::PermissionReq,
            0x54 => Self::AgentMeta,
            0x60 => Self::FeedbackEvent,
            0xFF => Self::Error,
            _ => return None,
        })
    }
}

/// SESSION_RESPONSE 的 status byte (与板端 SESSION_* 常量对齐)。
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatusByte {
    Ok = 0x00,
    Created = 0x01,
    InvalidS = 0x02,
    Expired = 0x03,
    PoolFull = 0x04,
    Reclaimed = 0x05,
    Disconnected = 0x06,
}

/// STATUS_UPDATE 的 state byte (与板端 SESSION_STATE_* 对齐)。
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStateByte {
    Connected = 0x00,
    Disconnected = 0x01,
    Run = 0x02,
    Wait = 0x03,
    Done = 0x04,
    Error = 0x05,
}

/// PERMISSION_RES 的 decision byte。
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecisionByte {
    Allow = 0x00,
    Deny = 0x01,
    Always = 0x02,
}

/// TURN_APPEND 的 role byte。
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnRoleByte {
    User = 0x00,
    Assistant = 0x01,
    Tool = 0x02,
    System = 0x03,
}

pub const SESSION_HEARTBEAT_TIMEOUT_MS: u64 = 30_000;
pub const SESSION_GC_TIMEOUT_MS: u64 = 60_000;
pub const SESSION_BROADCAST: BoardSid = BoardSid::BROADCAST;
pub const MAX_SESSIONS: usize = 256;
pub const PLUGIN_HINT_MAX: usize = 24;
pub const KEY_COUNT: usize = 7;
pub const ENCODER_STEPS_PER_EVENT: i32 = 2;

pub mod codec;
pub mod payloads;
pub use codec::{build_board_to_host, split_host_to_board, FrameError, HidFrame};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_roundtrip_known_values() {
        for raw in [
            0x01u8, 0x02, 0x03, 0x04, 0x05, 0x10, 0x11, 0x12, 0x13, 0x14, 0x30, 0x40, 0x50, 0x51,
            0x52, 0x53, 0x54, 0x60, 0xFF,
        ] {
            assert_eq!(Cmd::from_u8(raw).map(|c| c as u8), Some(raw));
        }
    }

    #[test]
    fn cmd_rejects_unknown() {
        assert!(Cmd::from_u8(0x99).is_none());
    }
}
