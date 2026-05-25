//! HID frame 编解码 + 长 payload 分帧。
//!
//! 与板端 `aikb_hid_input.c` 的 packet 结构对齐:
//!
//! ```text
//! [0]   report_id        u8
//! [1]   command          u8
//! [2-3] session_id       u16, little-endian
//! [4-5] payload_length   u16, little-endian
//! [6..] payload          ≤ 58 字节
//! ```

use crate::{
    Cmd, HID_HEADER_SIZE, HID_MAX_PAYLOAD, HID_REPORT_LEN, REPORT_ID_DEVICE_BOUND,
    REPORT_ID_HOST_BOUND,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidFrame {
    pub report_id: u8,
    pub cmd: Cmd,
    pub sid: u16,
    pub payload: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    TooShort,
    PayloadTooLarge,
    UnknownCmd(u8),
    HeaderLenMismatch,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => write!(f, "frame shorter than HID header"),
            Self::PayloadTooLarge => {
                write!(f, "payload exceeds HID_MAX_PAYLOAD ({HID_MAX_PAYLOAD})")
            }
            Self::UnknownCmd(c) => write!(f, "unknown command byte: 0x{c:02x}"),
            Self::HeaderLenMismatch => write!(f, "header length exceeds frame size"),
        }
    }
}

impl std::error::Error for FrameError {}

impl HidFrame {
    /// 单帧编码: 总是产出 HID_REPORT_LEN (64) 字节, payload 之后补零。
    pub fn encode(&self) -> Result<[u8; HID_REPORT_LEN], FrameError> {
        if self.payload.len() > HID_MAX_PAYLOAD {
            return Err(FrameError::PayloadTooLarge);
        }
        let mut buf = [0u8; HID_REPORT_LEN];
        buf[0] = self.report_id;
        buf[1] = self.cmd as u8;
        buf[2..4].copy_from_slice(&self.sid.to_le_bytes());
        let len = self.payload.len() as u16;
        buf[4..6].copy_from_slice(&len.to_le_bytes());
        buf[HID_HEADER_SIZE..HID_HEADER_SIZE + self.payload.len()].copy_from_slice(&self.payload);
        Ok(buf)
    }

    pub fn decode(data: &[u8]) -> Result<Self, FrameError> {
        if data.len() < HID_HEADER_SIZE {
            return Err(FrameError::TooShort);
        }
        let report_id = data[0];
        let cmd = Cmd::from_u8(data[1]).ok_or(FrameError::UnknownCmd(data[1]))?;
        let sid = u16::from_le_bytes([data[2], data[3]]);
        let len = u16::from_le_bytes([data[4], data[5]]) as usize;
        if HID_HEADER_SIZE + len > data.len() {
            return Err(FrameError::HeaderLenMismatch);
        }
        Ok(Self {
            report_id,
            cmd,
            sid,
            payload: data[HID_HEADER_SIZE..HID_HEADER_SIZE + len].to_vec(),
        })
    }
}

/// 把长 payload 分成多帧 (HOST→BOARD 方向)。VT100_STREAM / TURN_APPEND 等用。
/// 调用方负责保证 cmd 是"可分包"的; KEY_EVENT/HEARTBEAT 这类不分包。
pub fn split_host_to_board(cmd: Cmd, sid: u16, payload: &[u8]) -> Vec<HidFrame> {
    if payload.is_empty() {
        return vec![HidFrame {
            report_id: REPORT_ID_DEVICE_BOUND,
            cmd,
            sid,
            payload: Vec::new(),
        }];
    }
    payload
        .chunks(HID_MAX_PAYLOAD)
        .map(|chunk| HidFrame {
            report_id: REPORT_ID_DEVICE_BOUND,
            cmd,
            sid,
            payload: chunk.to_vec(),
        })
        .collect()
}

/// BOARD→HOST 方向的构造器, 例如 KEY_EVENT / PERMISSION_RES 都是单帧。
pub fn build_board_to_host(cmd: Cmd, sid: u16, payload: Vec<u8>) -> Result<HidFrame, FrameError> {
    if payload.len() > HID_MAX_PAYLOAD {
        return Err(FrameError::PayloadTooLarge);
    }
    Ok(HidFrame {
        report_id: REPORT_ID_HOST_BOUND,
        cmd,
        sid,
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_empty_payload() {
        let frame = HidFrame {
            report_id: REPORT_ID_DEVICE_BOUND,
            cmd: Cmd::SessionHeartbeat,
            sid: 7,
            payload: Vec::new(),
        };
        let bytes = frame.encode().unwrap();
        assert_eq!(bytes.len(), HID_REPORT_LEN);
        let decoded = HidFrame::decode(&bytes).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn roundtrip_max_payload() {
        let payload = vec![0xAB; HID_MAX_PAYLOAD];
        let frame = HidFrame {
            report_id: REPORT_ID_DEVICE_BOUND,
            cmd: Cmd::Vt100Stream,
            sid: 1,
            payload: payload.clone(),
        };
        let bytes = frame.encode().unwrap();
        let decoded = HidFrame::decode(&bytes).unwrap();
        assert_eq!(decoded.payload, payload);
    }

    #[test]
    fn rejects_oversize_payload() {
        let frame = HidFrame {
            report_id: REPORT_ID_DEVICE_BOUND,
            cmd: Cmd::TurnAppend,
            sid: 1,
            payload: vec![0; HID_MAX_PAYLOAD + 1],
        };
        assert_eq!(frame.encode(), Err(FrameError::PayloadTooLarge));
    }

    #[test]
    fn decode_too_short() {
        assert_eq!(HidFrame::decode(&[0u8; 3]), Err(FrameError::TooShort));
    }

    #[test]
    fn decode_unknown_cmd() {
        let mut bytes = [0u8; HID_REPORT_LEN];
        bytes[1] = 0x99;
        assert_eq!(HidFrame::decode(&bytes), Err(FrameError::UnknownCmd(0x99)));
    }

    #[test]
    fn split_short_payload_one_frame() {
        let frames = split_host_to_board(Cmd::TurnAppend, 5, b"hello world");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].report_id, REPORT_ID_DEVICE_BOUND);
        assert_eq!(frames[0].sid, 5);
        assert_eq!(frames[0].cmd as u8, Cmd::TurnAppend as u8);
        assert_eq!(frames[0].payload, b"hello world");
    }

    #[test]
    fn board_to_host_uses_host_bound_report_id() {
        let frame = build_board_to_host(Cmd::SessionFocus, 9, Vec::new()).unwrap();
        assert_eq!(frame.report_id, REPORT_ID_HOST_BOUND);
        assert_eq!(frame.sid, 9);
    }

    #[test]
    fn split_large_payload_chunks_at_58() {
        let payload = vec![0x42; HID_MAX_PAYLOAD * 2 + 10];
        let frames = split_host_to_board(Cmd::Vt100Stream, 1, &payload);
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].payload.len(), HID_MAX_PAYLOAD);
        assert_eq!(frames[1].payload.len(), HID_MAX_PAYLOAD);
        assert_eq!(frames[2].payload.len(), 10);
        let rebuilt: Vec<u8> = frames.iter().flat_map(|f| f.payload.clone()).collect();
        assert_eq!(rebuilt, payload);
    }

    #[test]
    fn split_empty_payload_yields_one_empty_frame() {
        let frames = split_host_to_board(Cmd::Vt100Stream, 1, b"");
        assert_eq!(frames.len(), 1);
        assert!(frames[0].payload.is_empty());
    }
}
