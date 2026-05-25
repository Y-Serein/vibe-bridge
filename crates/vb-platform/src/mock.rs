//! 内存 mock Platform, 供单测使用。

use std::cell::RefCell;

use vb_core::TerminalWindow;

use crate::{KeyStroke, Platform, PlatformError, ProcessInfo, Signal, WindowHandle};

#[derive(Debug, Default)]
pub struct MockPlatform {
    pub processes: RefCell<Vec<ProcessInfo>>,
    pub windows: RefCell<Vec<TerminalWindow>>,
    pub focus_calls: RefCell<Vec<WindowHandle>>,
    pub keystrokes: RefCell<Vec<(WindowHandle, KeyStroke)>>,
    pub signals: RefCell<Vec<(u32, Signal)>>,
}

impl Platform for MockPlatform {
    fn enumerate_processes(&self) -> Result<Vec<ProcessInfo>, PlatformError> {
        Ok(self.processes.borrow().clone())
    }

    fn enumerate_terminal_windows(&self) -> Result<Vec<TerminalWindow>, PlatformError> {
        Ok(self.windows.borrow().clone())
    }

    fn focus_window(&self, hwnd: WindowHandle) -> Result<(), PlatformError> {
        self.focus_calls.borrow_mut().push(hwnd);
        Ok(())
    }

    fn send_keystroke(&self, target: WindowHandle, keys: &KeyStroke) -> Result<(), PlatformError> {
        self.keystrokes.borrow_mut().push((target, keys.clone()));
        Ok(())
    }

    fn send_signal(&self, pid: u32, sig: Signal) -> Result<(), PlatformError> {
        self.signals.borrow_mut().push((pid, sig));
        Ok(())
    }
}
