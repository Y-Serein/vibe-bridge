//! Linux 平台实现 (M2 第一阶段, 不引入额外 crate)。
//!
//! 已实现:
//! - `enumerate_processes`: 解析 `/proc/<pid>/{comm,exe}`
//! - `send_signal`: 子进程调 `kill`, 避免引入 libc
//!
//! 占位 (后续小阶段引入 xdotool / wlroots / wayland 子进程支持):
//! - `enumerate_terminal_windows`
//! - `focus_window`
//! - `send_keystroke`

use std::fs;
use std::process::Command;

use vb_core::TerminalWindow;

use crate::{KeyStroke, Platform, PlatformError, ProcessInfo, Signal, WindowHandle};

pub struct LinuxPlatform;

impl LinuxPlatform {
    pub const fn new() -> Self {
        Self
    }
}

impl Default for LinuxPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl Platform for LinuxPlatform {
    fn enumerate_processes(&self) -> Result<Vec<ProcessInfo>, PlatformError> {
        let mut procs = Vec::new();
        let dir = fs::read_dir("/proc").map_err(|e| PlatformError::Io(e.to_string()))?;
        for entry in dir {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            let pid: u32 = match name_str.parse() {
                Ok(p) => p,
                Err(_) => continue,
            };

            let comm = fs::read_to_string(format!("/proc/{pid}/comm"))
                .unwrap_or_default()
                .trim()
                .to_string();
            let exe = fs::read_link(format!("/proc/{pid}/exe"))
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();

            if comm.is_empty() && exe.is_empty() {
                continue;
            }

            procs.push(ProcessInfo {
                pid,
                name: comm,
                path: exe,
            });
        }
        Ok(procs)
    }

    fn enumerate_terminal_windows(&self) -> Result<Vec<TerminalWindow>, PlatformError> {
        Err(PlatformError::Unsupported(
            "Linux terminal enumeration pending xdotool/wlroots integration",
        ))
    }

    fn focus_window(&self, _hwnd: WindowHandle) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported(
            "Linux focus_window pending xdotool/wlroots integration",
        ))
    }

    fn send_keystroke(
        &self,
        _target: WindowHandle,
        _keys: &KeyStroke,
    ) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported(
            "Linux send_keystroke pending xdotool/ydotool integration",
        ))
    }

    fn send_signal(&self, pid: u32, sig: Signal) -> Result<(), PlatformError> {
        let signame = match sig {
            Signal::Interrupt => "-INT",
            Signal::Terminate => "-TERM",
        };
        let status = Command::new("kill")
            .arg(signame)
            .arg(pid.to_string())
            .status()
            .map_err(|e| PlatformError::Io(e.to_string()))?;
        if status.success() {
            Ok(())
        } else {
            Err(PlatformError::NotFound)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerates_self_process() {
        let plat = LinuxPlatform::new();
        let procs = plat.enumerate_processes().unwrap();
        let self_pid = std::process::id();
        assert!(
            procs.iter().any(|p| p.pid == self_pid),
            "expected to see our own pid {self_pid} in /proc enumeration"
        );
    }

    #[test]
    fn terminal_and_gui_are_unsupported_for_now() {
        let plat = LinuxPlatform::new();
        assert!(plat.enumerate_terminal_windows().is_err());
        assert!(plat.focus_window(WindowHandle { raw: 0, pid: 0 }).is_err());
        assert!(plat
            .send_keystroke(
                WindowHandle { raw: 0, pid: 0 },
                &KeyStroke {
                    text: "x".to_string()
                }
            )
            .is_err());
    }
}
