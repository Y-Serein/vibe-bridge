//! Transport 抽象层。
//!
//! 三类传输 (M2-M3 实现):
//! - [`HidTransport`]: 与 AIKB 板端通过 HID 收发 (hidapi crate, 跨 Win/Mac/Linux)
//! - [`LocalIpc`]: agent → daemon 注册/活动事件 (Unix socket / Windows named pipe)
//! - [`ChannelTransport`]: tokio mpsc, 单元测试用
//!
//! 当前 M1 只定义 trait 与 placeholder, 让 workspace 编译通过; 实际收发逻辑后续填。

use vb_protocol::Cmd;

/// 一个 HID 方向上的单条消息 (命令 + sid + payload)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidMessage {
    pub cmd: Cmd,
    pub sid: u16,
    pub payload: Vec<u8>,
}

/// HID transport trait — M2 由 hidapi crate 实现。
pub trait HidTransport {
    fn send(&self, msg: &HidMessage) -> Result<(), TransportError>;
    fn recv(&self) -> Result<HidMessage, TransportError>;
    fn is_connected(&self) -> bool;
}

/// Local IPC trait — daemon 接收 agent 注册/活动 (M3 实现)。
pub trait LocalIpc {
    fn serve(&self) -> Result<(), TransportError>;
}

#[derive(Debug)]
pub enum TransportError {
    NotConnected,
    Io(String),
    Decode(String),
    Timeout,
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConnected => write!(f, "transport not connected"),
            Self::Io(msg) => write!(f, "transport io error: {msg}"),
            Self::Decode(msg) => write!(f, "transport decode error: {msg}"),
            Self::Timeout => write!(f, "transport timeout"),
        }
    }
}

impl std::error::Error for TransportError {}

#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
pub use windows::{
    resolve_win_hid_device, ReopenWinHidTransport, WinHidDeviceInfo, WinHidTransport,
};
