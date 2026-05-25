//! Windows 平台实现。
//!
//! 这一层只封装低层 OS 动作: 进程枚举、顶层终端窗口枚举、聚焦、文本输入和中止。
//! Windows Terminal tab/pane 级 UI Automation 仍由 `vb-host` 的 discovery 层处理。

use std::mem::{size_of, zeroed};

use vb_core::{TerminalKind, TerminalWindow};
use windows_sys::Win32::Foundation::{CloseHandle, BOOL, HANDLE, HWND, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Console::{
    AttachConsole, FreeConsole, GenerateConsoleCtrlEvent, CTRL_C_EVENT,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    SetForegroundWindow,
};

use crate::{KeyStroke, Platform, PlatformError, ProcessInfo, Signal, WindowHandle};

pub struct WindowsPlatform;

impl WindowsPlatform {
    pub const fn new() -> Self {
        Self
    }
}

impl Default for WindowsPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl Platform for WindowsPlatform {
    fn enumerate_processes(&self) -> Result<Vec<ProcessInfo>, PlatformError> {
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(PlatformError::Io("CreateToolhelp32Snapshot failed".into()));
            }

            let mut entry: PROCESSENTRY32W = zeroed();
            entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            let mut out = Vec::new();
            let mut ok = Process32FirstW(snapshot, &mut entry);
            while ok != 0 {
                let name = wide_z_to_string(&entry.szExeFile);
                let path = query_process_path(entry.th32ProcessID).unwrap_or_default();
                out.push(ProcessInfo {
                    pid: entry.th32ProcessID,
                    name,
                    path,
                });
                ok = Process32NextW(snapshot, &mut entry);
            }
            CloseHandle(snapshot);
            Ok(out)
        }
    }

    fn enumerate_terminal_windows(&self) -> Result<Vec<TerminalWindow>, PlatformError> {
        let mut windows = Vec::<TerminalWindow>::new();
        unsafe {
            let data = &mut windows as *mut Vec<TerminalWindow> as isize;
            EnumWindows(Some(enum_window_proc), data);
        }
        Ok(windows)
    }

    fn focus_window(&self, hwnd: WindowHandle) -> Result<(), PlatformError> {
        unsafe {
            if SetForegroundWindow(hwnd.raw as HWND) != 0 {
                Ok(())
            } else {
                Err(PlatformError::NotFound)
            }
        }
    }

    fn send_keystroke(&self, target: WindowHandle, keys: &KeyStroke) -> Result<(), PlatformError> {
        self.focus_window(target)?;
        let mut inputs = Vec::new();
        for unit in keys.text.encode_utf16() {
            inputs.push(key_input(unit, false));
            inputs.push(key_input(unit, true));
        }
        if inputs.is_empty() {
            return Ok(());
        }
        unsafe {
            let sent = SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                size_of::<INPUT>() as i32,
            );
            if sent == inputs.len() as u32 {
                Ok(())
            } else {
                Err(PlatformError::Io(format!(
                    "SendInput sent {sent}/{} events",
                    inputs.len()
                )))
            }
        }
    }

    fn send_signal(&self, pid: u32, sig: Signal) -> Result<(), PlatformError> {
        match sig {
            Signal::Interrupt => send_ctrl_c(pid),
            Signal::Terminate => terminate_process(pid),
        }
    }
}

unsafe extern "system" fn enum_window_proc(hwnd: HWND, lparam: isize) -> BOOL {
    if IsWindowVisible(hwnd) == 0 {
        return 1;
    }
    let title_len = GetWindowTextLengthW(hwnd);
    if title_len <= 0 {
        return 1;
    }

    let mut title_buf = vec![0u16; title_len as usize + 1];
    let copied = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), title_buf.len() as i32);
    if copied <= 0 {
        return 1;
    }
    title_buf.truncate(copied as usize);
    let title = String::from_utf16_lossy(&title_buf);
    let lower_title = title.to_ascii_lowercase();

    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid == 0 {
        return 1;
    }
    let process_path = query_process_path(pid).unwrap_or_default();
    let lower_path = process_path.to_ascii_lowercase();
    let kind = classify_terminal(&lower_title, &lower_path);
    if kind == TerminalKind::Unknown {
        return 1;
    }

    let windows = &mut *(lparam as *mut Vec<TerminalWindow>);
    windows.push(TerminalWindow {
        hwnd: hwnd as usize,
        pid,
        title,
        process_path,
        kind,
    });
    1
}

fn key_input(unit: u16, key_up: bool) -> INPUT {
    let flags = if key_up {
        KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
    } else {
        KEYEVENTF_UNICODE
    };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: unit,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn classify_terminal(title: &str, path: &str) -> TerminalKind {
    if path.contains("windowsterminal") || title.contains("windows terminal") {
        TerminalKind::WindowsTerminal
    } else if path.contains("conhost") {
        TerminalKind::ConsoleHost
    } else if path.contains("powershell") || path.contains("pwsh") {
        TerminalKind::PowerShell
    } else if path.contains("\\cmd.exe") || path.ends_with("/cmd.exe") {
        TerminalKind::Cmd
    } else if path.contains("\\wsl.exe") || title.contains("@") {
        TerminalKind::Wsl
    } else if title.contains("claude") || title.contains("codex") {
        TerminalKind::TerminalLike
    } else {
        TerminalKind::Unknown
    }
}

fn wide_z_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn query_process_path(pid: u32) -> Result<String, PlatformError> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return Err(PlatformError::NotFound);
        }
        let mut buf = vec![0u16; 32768];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(handle);
        if ok == 0 {
            return Err(PlatformError::NotFound);
        }
        buf.truncate(len as usize);
        Ok(String::from_utf16_lossy(&buf))
    }
}

fn send_ctrl_c(pid: u32) -> Result<(), PlatformError> {
    unsafe {
        if AttachConsole(pid) == 0 {
            return Err(PlatformError::NotFound);
        }
        let ok = GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0);
        FreeConsole();
        if ok == 0 {
            Err(PlatformError::Io("GenerateConsoleCtrlEvent failed".into()))
        } else {
            Ok(())
        }
    }
}

fn terminate_process(pid: u32) -> Result<(), PlatformError> {
    unsafe {
        let handle: HANDLE = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle.is_null() {
            return Err(PlatformError::NotFound);
        }
        let ok = TerminateProcess(handle, 1);
        CloseHandle(handle);
        if ok == 0 {
            Err(PlatformError::Io("TerminateProcess failed".into()))
        } else {
            Ok(())
        }
    }
}
