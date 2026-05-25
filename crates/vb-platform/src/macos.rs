//! macOS 平台实现骨架。
//!
//! M2 第一阶段只暴露 trait 接口, 所有方法返回 Unsupported。
//! M2 后续小阶段引入 `core-foundation` / `core-graphics` / `objc` crate, 计划实现:
//!
//! - `enumerate_processes`: `libproc` 或 `sysctl(KERN_PROC)`
//! - `enumerate_terminal_windows`: `CGWindowListCopyWindowInfo`
//! - `focus_window`: AppleScript via `osascript` 子进程, 或 Accessibility API
//! - `send_keystroke`: `CGEventCreateKeyboardEvent` + `CGEventPost`
//! - `send_signal`:
//!   - `Signal::Interrupt` → SIGINT via libc::kill
//!   - `Signal::Terminate` → SIGTERM via libc::kill

use vb_core::TerminalWindow;

use crate::{KeyStroke, Platform, PlatformError, ProcessInfo, Signal, WindowHandle};

pub struct MacOsPlatform;

impl MacOsPlatform {
    pub const fn new() -> Self {
        Self
    }
}

impl Default for MacOsPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl Platform for MacOsPlatform {
    fn enumerate_processes(&self) -> Result<Vec<ProcessInfo>, PlatformError> {
        Err(PlatformError::Unsupported(
            "macOS enumerate_processes pending libproc/sysctl binding",
        ))
    }

    fn enumerate_terminal_windows(&self) -> Result<Vec<TerminalWindow>, PlatformError> {
        Err(PlatformError::Unsupported(
            "macOS enumerate_terminal_windows pending CGWindowListCopyWindowInfo binding",
        ))
    }

    fn focus_window(&self, _hwnd: WindowHandle) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported(
            "macOS focus_window pending AppleScript/Accessibility binding",
        ))
    }

    fn send_keystroke(
        &self,
        _target: WindowHandle,
        _keys: &KeyStroke,
    ) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported(
            "macOS send_keystroke pending CGEvent binding",
        ))
    }

    fn send_signal(&self, _pid: u32, _sig: Signal) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported(
            "macOS send_signal pending libc::kill binding",
        ))
    }
}
