//! OS 抽象层 (Platform trait)。
//!
//! 隔离 Win32 / macOS / Linux 差异,让 daemon / host 业务层不感知具体 OS。
//! M1 只定义 trait + Mock; M2 填三平台实现 (windows.rs / macos.rs / linux.rs)。

use vb_core::TerminalWindow;

/// 跨平台进程信息 (轻量，不带 environ/cmdline)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub path: String,
}

/// 窗口句柄。Windows HWND / macOS NSWindow / Linux X11 Window 统一抽象。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowHandle {
    pub raw: usize,
    pub pid: u32,
}

/// 要发送的按键序列 (M2 完善, 当前先支持 text)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyStroke {
    pub text: String,
}

/// 信号语义 (Unix SIGINT/SIGTERM / Windows GenerateConsoleCtrlEvent CTRL_C/BREAK)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Interrupt,
    Terminate,
}

#[derive(Debug)]
pub enum PlatformError {
    Unsupported(&'static str),
    Io(String),
    NotFound,
}

impl std::fmt::Display for PlatformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(what) => write!(f, "platform unsupported: {what}"),
            Self::Io(msg) => write!(f, "platform io: {msg}"),
            Self::NotFound => write!(f, "platform target not found"),
        }
    }
}

impl std::error::Error for PlatformError {}

pub trait Platform {
    fn enumerate_processes(&self) -> Result<Vec<ProcessInfo>, PlatformError>;
    fn enumerate_terminal_windows(&self) -> Result<Vec<TerminalWindow>, PlatformError>;
    fn focus_window(&self, hwnd: WindowHandle) -> Result<(), PlatformError>;
    fn send_keystroke(&self, target: WindowHandle, keys: &KeyStroke) -> Result<(), PlatformError>;
    fn send_signal(&self, pid: u32, sig: Signal) -> Result<(), PlatformError>;
}

pub mod mock;
pub use mock::MockPlatform;

#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "windows")]
pub use windows::WindowsPlatform as NativePlatform;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub use macos::MacOsPlatform as NativePlatform;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub use linux::LinuxPlatform as NativePlatform;
