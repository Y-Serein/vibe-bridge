//! Native Windows ConPTY wrapper for `vb-daemon launch`.
//!
//! Mirrors the structure of the (deprecated) Python `windows_runner._run_conpty`:
//! create input + output pipes, build a pseudoconsole, attach it to a child
//! process via STARTUPINFOEXW + PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, then expose
//! the read/write handles so the caller can pump bytes both ways.
//!
//! Only compiled on Windows. The unit tests in `lib.rs` exercise the JSON glue;
//! ConPTY itself needs a live Win32 console to spawn, so it is covered by
//! manual on-Windows verification rather than `cargo test`.

#![cfg(windows)]

use std::ffi::{c_void, OsStr, OsString};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, BOOL, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows_sys::Win32::System::Console::{
    ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, UpdateProcThreadAttribute, WaitForSingleObject,
    CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, INFINITE,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, STARTUPINFOEXW, STARTUPINFOW,
};

const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x0002_0016;

/// Owning handle for an extended STARTUPINFOW attribute list. The buffer must
/// outlive `CreateProcessW`; `Drop` releases the kernel-side bookkeeping.
struct AttributeList {
    #[allow(dead_code)]
    buf: Vec<u8>,
    raw: LPPROC_THREAD_ATTRIBUTE_LIST,
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe { DeleteProcThreadAttributeList(self.raw) };
            self.raw = ptr::null_mut();
        }
    }
}

/// Live ConPTY session: holds the read end of stdout, the write end of stdin,
/// the pseudoconsole handle and the spawned child process so the caller can
/// pump bytes and wait for exit.
pub struct ConPtySession {
    stdin_write: HANDLE,
    stdout_read: HANDLE,
    pty: HPCON,
    process: HANDLE,
    thread: HANDLE,
    #[allow(dead_code)]
    attrs: AttributeList,
}

unsafe impl Send for ConPtySession {}
unsafe impl Sync for ConPtySession {}

impl ConPtySession {
    /// Spawn `argv` under a freshly-created pseudoconsole sized `cols x rows`.
    pub fn spawn(argv: &[OsString], cols: i16, rows: i16) -> io::Result<Self> {
        if argv.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "argv is empty"));
        }

        unsafe {
            let (in_read, in_write) = create_pipe()?;
            let (out_read, out_write) = match create_pipe() {
                Ok(p) => p,
                Err(err) => {
                    let _ = CloseHandle(in_read);
                    let _ = CloseHandle(in_write);
                    return Err(err);
                }
            };

            let mut pty: HPCON = 0;
            let size = COORD { X: cols, Y: rows };
            let hr = CreatePseudoConsole(size, in_read, out_write, 0, &mut pty);
            // Whether or not the call succeeded, the pipe ends we passed are
            // now owned by the pseudoconsole; close our duplicates.
            let _ = CloseHandle(in_read);
            let _ = CloseHandle(out_write);
            if hr != 0 {
                let _ = CloseHandle(in_write);
                let _ = CloseHandle(out_read);
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("CreatePseudoConsole failed: 0x{hr:08x}"),
                ));
            }

            let attrs = match build_attribute_list(pty) {
                Ok(a) => a,
                Err(err) => {
                    ClosePseudoConsole(pty);
                    let _ = CloseHandle(in_write);
                    let _ = CloseHandle(out_read);
                    return Err(err);
                }
            };

            let mut si: STARTUPINFOEXW = std::mem::zeroed();
            si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            si.lpAttributeList = attrs.raw;

            let mut command_line = build_command_line(argv);
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let ok = CreateProcessW(
                ptr::null(),
                command_line.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                ptr::null_mut(),
                ptr::null(),
                &si as *const _ as *const STARTUPINFOW,
                &mut pi,
            );
            if ok == 0 {
                let err = last_error("CreateProcessW");
                ClosePseudoConsole(pty);
                let _ = CloseHandle(in_write);
                let _ = CloseHandle(out_read);
                return Err(err);
            }

            Ok(Self {
                stdin_write: in_write,
                stdout_read: out_read,
                pty,
                process: pi.hProcess,
                thread: pi.hThread,
                attrs,
            })
        }
    }

    /// Blocking read of up to `buf.len()` bytes from the child's stdout. Returns
    /// `Ok(0)` on EOF (child closed the pipe).
    pub fn read_output(&self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut read: u32 = 0;
        let ok: BOOL = unsafe {
            ReadFile(
                self.stdout_read,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut read,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            let code = unsafe { GetLastError() };
            // ERROR_BROKEN_PIPE = child closed its output handle.
            if code == 109 {
                return Ok(0);
            }
            return Err(io::Error::from_raw_os_error(code as i32));
        }
        Ok(read as usize)
    }

    /// Write `data` into the child's stdin. Loops until the entire slice is
    /// written or an error occurs.
    pub fn write_input(&self, data: &[u8]) -> io::Result<()> {
        let mut written_total = 0;
        while written_total < data.len() {
            let mut written: u32 = 0;
            let chunk = &data[written_total..];
            let ok: BOOL = unsafe {
                WriteFile(
                    self.stdin_write,
                    chunk.as_ptr(),
                    chunk.len() as u32,
                    &mut written,
                    ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(io::Error::from_raw_os_error(
                    unsafe { GetLastError() } as i32
                ));
            }
            written_total += written as usize;
        }
        Ok(())
    }

    /// Block until the child process exits and return its exit code.
    pub fn wait(&self) -> io::Result<u32> {
        let rc = unsafe { WaitForSingleObject(self.process, INFINITE) };
        if rc != WAIT_OBJECT_0 {
            return Err(io::Error::from_raw_os_error(
                unsafe { GetLastError() } as i32
            ));
        }
        let mut code: u32 = 1;
        let ok = unsafe { GetExitCodeProcess(self.process, &mut code) };
        if ok == 0 {
            return Err(io::Error::from_raw_os_error(
                unsafe { GetLastError() } as i32
            ));
        }
        Ok(code)
    }

    /// Resize the pseudoconsole to match the host terminal window.
    pub fn resize(&self, cols: i16, rows: i16) -> io::Result<()> {
        let hr = unsafe { ResizePseudoConsole(self.pty, COORD { X: cols, Y: rows }) };
        if hr != 0 {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("ResizePseudoConsole failed: 0x{hr:08x}"),
            ));
        }
        Ok(())
    }
}

impl Drop for ConPtySession {
    fn drop(&mut self) {
        unsafe {
            // Close stdin first so the child sees EOF and the output pipe can
            // drain.
            if is_valid_handle(self.stdin_write) {
                let _ = CloseHandle(self.stdin_write);
            }
            if self.pty != 0 {
                ClosePseudoConsole(self.pty);
            }
            if is_valid_handle(self.stdout_read) {
                let _ = CloseHandle(self.stdout_read);
            }
            if is_valid_handle(self.thread) {
                let _ = CloseHandle(self.thread);
            }
            if is_valid_handle(self.process) {
                let _ = CloseHandle(self.process);
            }
        }
    }
}

unsafe fn build_attribute_list(pty: HPCON) -> io::Result<AttributeList> {
    let mut size: usize = 0;
    InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size);
    // First call always "fails" with ERROR_INSUFFICIENT_BUFFER and writes the
    // required size; that is expected — only treat zero size as an error.
    if size == 0 {
        return Err(last_error("InitializeProcThreadAttributeList(size probe)"));
    }
    let mut buf = vec![0u8; size];
    let attr_ptr = buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
    if InitializeProcThreadAttributeList(attr_ptr, 1, 0, &mut size) == 0 {
        return Err(last_error("InitializeProcThreadAttributeList"));
    }
    if UpdateProcThreadAttribute(
        attr_ptr,
        0,
        PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
        pty as *const c_void,
        std::mem::size_of::<HPCON>(),
        ptr::null_mut(),
        ptr::null_mut(),
    ) == 0
    {
        let err = last_error("UpdateProcThreadAttribute");
        DeleteProcThreadAttributeList(attr_ptr);
        return Err(err);
    }
    Ok(AttributeList { buf, raw: attr_ptr })
}

fn is_valid_handle(handle: HANDLE) -> bool {
    !handle.is_null() && handle != INVALID_HANDLE_VALUE
}

unsafe fn create_pipe() -> io::Result<(HANDLE, HANDLE)> {
    let mut read: HANDLE = INVALID_HANDLE_VALUE;
    let mut write: HANDLE = INVALID_HANDLE_VALUE;
    let mut sa: SECURITY_ATTRIBUTES = std::mem::zeroed();
    sa.nLength = std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32;
    sa.bInheritHandle = 1;
    if CreatePipe(&mut read, &mut write, &mut sa, 0) == 0 {
        return Err(last_error("CreatePipe"));
    }
    Ok((read, write))
}

fn build_command_line(argv: &[OsString]) -> Vec<u16> {
    let mut s = String::new();
    for (i, arg) in argv.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        // Quote arguments containing whitespace; preserve backslashes verbatim.
        let arg_lossy = arg.to_string_lossy();
        if arg_lossy.contains(' ') || arg_lossy.contains('\t') {
            s.push('"');
            for ch in arg_lossy.chars() {
                if ch == '"' {
                    s.push('\\');
                }
                s.push(ch);
            }
            s.push('"');
        } else {
            s.push_str(&arg_lossy);
        }
    }
    let mut wide: Vec<u16> = OsStr::new(&s).encode_wide().collect();
    wide.push(0);
    wide
}

fn last_error(context: &str) -> io::Error {
    let code = unsafe { GetLastError() };
    io::Error::new(
        io::ErrorKind::Other,
        format!("{context} failed: GetLastError=0x{code:08x}"),
    )
}
