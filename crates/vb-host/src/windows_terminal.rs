#![cfg(windows)]

use std::mem::MaybeUninit;
use std::process::Command;

use vb_core::{classify_terminal_process, TerminalWindow};

type Bool = i32;
type Dword = u32;
type Hwnd = isize;
type Handle = isize;
type Lparam = isize;

const PROCESS_QUERY_LIMITED_INFORMATION: Dword = 0x1000;
const SW_RESTORE: i32 = 9;

#[link(name = "user32")]
extern "system" {
    fn EnumWindows(callback: extern "system" fn(Hwnd, Lparam) -> Bool, lparam: Lparam) -> Bool;
    fn IsWindowVisible(hwnd: Hwnd) -> Bool;
    fn GetWindowTextLengthW(hwnd: Hwnd) -> i32;
    fn GetWindowTextW(hwnd: Hwnd, text: *mut u16, max_count: i32) -> i32;
    fn GetWindowThreadProcessId(hwnd: Hwnd, process_id: *mut Dword) -> Dword;
    fn SetForegroundWindow(hwnd: Hwnd) -> Bool;
    fn ShowWindow(hwnd: Hwnd, command_show: i32) -> Bool;
}

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(desired_access: Dword, inherit_handle: Bool, process_id: Dword) -> Handle;
    fn CloseHandle(handle: Handle) -> Bool;
    fn QueryFullProcessImageNameW(
        process: Handle,
        flags: Dword,
        exe_name: *mut u16,
        size: *mut Dword,
    ) -> Bool;
}

pub fn discover_terminal_windows() -> Result<Vec<TerminalWindow>, String> {
    let mut out = Vec::<TerminalWindow>::new();
    let ok = unsafe {
        EnumWindows(
            enum_windows_callback,
            (&mut out as *mut Vec<TerminalWindow>) as Lparam,
        )
    };
    if ok == 0 {
        return Err("EnumWindows failed".to_string());
    }
    Ok(out)
}

pub fn focus_window(hwnd: usize) -> Result<(), String> {
    let hwnd = hwnd as Hwnd;
    unsafe {
        ShowWindow(hwnd, SW_RESTORE);
        if SetForegroundWindow(hwnd) == 0 {
            return Err("SetForegroundWindow failed".to_string());
        }
    }
    Ok(())
}

pub fn discover_terminal_titles() -> Result<Vec<String>, String> {
    let tabs = discover_windows_terminal_tab_titles().unwrap_or_default();
    if !tabs.is_empty() {
        return Ok(tabs);
    }
    discover_terminal_windows().map(|windows| {
        windows
            .into_iter()
            .map(|window| window.title)
            .filter(|title| !title.trim().is_empty())
            .collect()
    })
}

extern "system" fn enum_windows_callback(hwnd: Hwnd, lparam: Lparam) -> Bool {
    let windows = unsafe { &mut *(lparam as *mut Vec<TerminalWindow>) };
    if unsafe { IsWindowVisible(hwnd) } == 0 {
        return 1;
    }

    let title = window_title(hwnd);
    if title.trim().is_empty() {
        return 1;
    }

    let pid = window_pid(hwnd);
    if pid == 0 {
        return 1;
    }

    let process_path = query_process_path(pid).unwrap_or_default();
    let kind = classify_terminal_process(&process_path, &title);
    let window = TerminalWindow {
        hwnd: hwnd as usize,
        pid,
        title,
        process_path,
        kind,
    };
    if window.is_terminal_candidate() {
        windows.push(window);
    }
    1
}

fn window_title(hwnd: Hwnd) -> String {
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; (len as usize) + 1];
    let copied = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if copied <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..copied as usize])
}

fn window_pid(hwnd: Hwnd) -> u32 {
    let mut pid = MaybeUninit::<Dword>::zeroed();
    unsafe {
        GetWindowThreadProcessId(hwnd, pid.as_mut_ptr());
        pid.assume_init()
    }
}

fn query_process_path(pid: u32) -> Option<String> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle == 0 {
        return None;
    }
    let result = query_process_path_handle(handle);
    unsafe {
        CloseHandle(handle);
    }
    result
}

fn query_process_path_handle(handle: Handle) -> Option<String> {
    let mut buf = vec![0u16; 32768];
    let mut size = buf.len() as Dword;
    let ok =
        unsafe { QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size as *mut Dword) };
    if ok == 0 || size == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..size as usize]))
}

fn discover_windows_terminal_tab_titles() -> Result<Vec<String>, String> {
    let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Add-Type -AssemblyName UIAutomationClient | Out-Null
Add-Type -AssemblyName UIAutomationTypes | Out-Null
$root = [System.Windows.Automation.AutomationElement]::RootElement
$windowCondition = New-Object System.Windows.Automation.PropertyCondition(
  [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
  [System.Windows.Automation.ControlType]::Window
)
$tabCondition = New-Object System.Windows.Automation.PropertyCondition(
  [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
  [System.Windows.Automation.ControlType]::TabItem
)
$out = New-Object System.Collections.Generic.List[string]
$windows = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $windowCondition)
foreach ($window in $windows) {
  $className = $window.Current.ClassName
  if ($className -ne 'CASCADIA_HOSTING_WINDOW_CLASS') { continue }
  $tabs = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, $tabCondition)
  foreach ($tab in $tabs) {
    $name = $tab.Current.Name
    if (-not [string]::IsNullOrWhiteSpace($name)) { $out.Add($name) }
  }
}
@($out.ToArray()) | ConvertTo-Json -Compress
"#;
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .output()
        .map_err(|err| format!("run PowerShell UIAutomation: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "PowerShell UIAutomation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    parse_json_string_list(&output.stdout)
}

fn parse_json_string_list(bytes: &[u8]) -> Result<Vec<String>, String> {
    let text = String::from_utf8_lossy(bytes).trim().to_string();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|err| format!("parse UIAutomation JSON: {err}"))?;
    match value {
        serde_json::Value::Array(items) => Ok(items
            .into_iter()
            .filter_map(|item| item.as_str().map(str::trim).map(ToOwned::to_owned))
            .filter(|item| !item.is_empty())
            .collect()),
        serde_json::Value::String(item) => {
            let item = item.trim().to_string();
            Ok(if item.is_empty() {
                Vec::new()
            } else {
                vec![item]
            })
        }
        _ => Ok(Vec::new()),
    }
}
