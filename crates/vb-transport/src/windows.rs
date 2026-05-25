use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use vb_protocol::codec::HidFrame;
use vb_protocol::{HID_REPORT_LEN, REPORT_ID_DEVICE_BOUND};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows_sys::Win32::Devices::HumanInterfaceDevice::HidD_GetHidGuid;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, GENERIC_READ,
    GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

use crate::{HidMessage, HidTransport, TransportError};

pub const VIBE_USB_VID: u16 = 0x359f;
pub const VIBE_USB_PID: u16 = 0x2120;
const HID_WRITE_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinHidDeviceInfo {
    pub path: String,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
}

pub struct WinHidTransport {
    handle: Mutex<HANDLE>,
}

unsafe impl Send for WinHidTransport {}
unsafe impl Sync for WinHidTransport {}

pub struct ReopenWinHidTransport {
    device: String,
    inner: Mutex<Option<Arc<WinHidTransport>>>,
}

unsafe impl Send for ReopenWinHidTransport {}
unsafe impl Sync for ReopenWinHidTransport {}

impl WinHidTransport {
    pub fn open(path: &str) -> Result<Self, TransportError> {
        let wide = wide_null(path);
        unsafe {
            let handle = CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                null_mut(),
            );
            if handle == INVALID_HANDLE_VALUE {
                return Err(last_error("CreateFileW"));
            }
            Ok(Self {
                handle: Mutex::new(handle),
            })
        }
    }

    pub fn open_auto() -> Result<Self, TransportError> {
        let path = resolve_win_hid_device()?
            .ok_or_else(|| TransportError::Io("no Vibe HID 359f:2120 device found".into()))?;
        Self::open(&path)
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<Option<HidMessage>, TransportError> {
        let mut buf = [0u8; HID_REPORT_LEN];
        let mut read = 0u32;
        let event = unsafe { CreateEventW(null(), 1, 0, null()) };
        if event.is_null() {
            return Err(last_error("CreateEventW"));
        }
        let mut overlapped: OVERLAPPED = unsafe { zeroed() };
        overlapped.hEvent = event;

        let result = (|| {
            let handle = *self
                .handle
                .lock()
                .map_err(|_| TransportError::Io("HID handle mutex poisoned".into()))?;
            unsafe {
                let ok = ReadFile(
                    handle,
                    buf.as_mut_ptr(),
                    HID_REPORT_LEN as u32,
                    &mut read,
                    &mut overlapped,
                );
                if ok == 0 {
                    let err = GetLastError();
                    if err != ERROR_IO_PENDING {
                        return Err(last_error_code("ReadFile", err));
                    }
                    let wait = WaitForSingleObject(event, duration_to_ms(timeout));
                    if wait == WAIT_TIMEOUT {
                        CancelIoEx(handle, &mut overlapped);
                        let mut cancelled = 0u32;
                        let _ = GetOverlappedResult(handle, &mut overlapped, &mut cancelled, 1);
                        return Ok(None);
                    }
                    if wait != WAIT_OBJECT_0 {
                        CancelIoEx(handle, &mut overlapped);
                        return Err(last_error("WaitForSingleObject"));
                    }
                    let ok = GetOverlappedResult(handle, &mut overlapped, &mut read, 0);
                    if ok == 0 {
                        let err = GetLastError();
                        if err == ERROR_OPERATION_ABORTED {
                            return Ok(None);
                        }
                        return Err(last_error_code("GetOverlappedResult", err));
                    }
                }
            }
            let frame =
                HidFrame::decode(&buf).map_err(|err| TransportError::Decode(err.to_string()))?;
            Ok(Some(HidMessage {
                cmd: frame.cmd,
                sid: frame.sid,
                payload: frame.payload,
            }))
        })();
        unsafe {
            CloseHandle(event);
        }
        result
    }
}

impl ReopenWinHidTransport {
    pub fn open(device: &str) -> Result<Self, TransportError> {
        let inner = Some(Arc::new(open_device(device)?));
        Ok(Self {
            device: device.to_string(),
            inner: Mutex::new(inner),
        })
    }

    fn open_current(&self) -> Result<WinHidTransport, TransportError> {
        open_device(&self.device)
    }

    fn invalidate(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            *inner = None;
        }
    }

    fn with_transport<R>(
        &self,
        mut op: impl FnMut(&WinHidTransport) -> Result<R, TransportError>,
    ) -> Result<R, TransportError> {
        for attempt in 0..2 {
            let transport = {
                let mut inner = self
                    .inner
                    .lock()
                    .map_err(|_| TransportError::Io("HID reopen mutex poisoned".into()))?;
                if inner.is_none() {
                    *inner = Some(Arc::new(self.open_current()?));
                }
                inner.clone().ok_or(TransportError::NotConnected)?
            };
            match op(&transport) {
                Ok(value) => return Ok(value),
                Err(err) => {
                    self.invalidate();
                    if attempt > 0 {
                        return Err(err);
                    }
                }
            }
        }
        Err(TransportError::NotConnected)
    }
}

impl HidTransport for ReopenWinHidTransport {
    fn send(&self, msg: &HidMessage) -> Result<(), TransportError> {
        self.with_transport(|transport| transport.send(msg))
    }

    fn recv(&self) -> Result<HidMessage, TransportError> {
        match self.with_transport(|transport| transport.recv()) {
            Ok(msg) => Ok(msg),
            Err(err) => {
                self.invalidate();
                Err(err)
            }
        }
    }

    fn is_connected(&self) -> bool {
        self.inner
            .lock()
            .map(|inner| {
                inner
                    .as_ref()
                    .is_some_and(|transport| transport.is_connected())
            })
            .unwrap_or(false)
    }
}

impl Drop for WinHidTransport {
    fn drop(&mut self) {
        if let Ok(mut handle) = self.handle.lock() {
            if !handle.is_null() && *handle != INVALID_HANDLE_VALUE {
                unsafe {
                    CloseHandle(*handle);
                }
                *handle = null_mut();
            }
        }
    }
}

impl HidTransport for WinHidTransport {
    fn send(&self, msg: &HidMessage) -> Result<(), TransportError> {
        let frame = HidFrame {
            report_id: REPORT_ID_DEVICE_BOUND,
            cmd: msg.cmd,
            sid: msg.sid,
            payload: msg.payload.clone(),
        };
        let raw = frame
            .encode()
            .map_err(|err| TransportError::Decode(err.to_string()))?;
        let mut written = 0u32;
        let event = unsafe { CreateEventW(null(), 1, 0, null()) };
        if event.is_null() {
            return Err(last_error("CreateEventW"));
        }
        let mut overlapped: OVERLAPPED = unsafe { zeroed() };
        overlapped.hEvent = event;
        let result = (|| {
            let handle = *self
                .handle
                .lock()
                .map_err(|_| TransportError::Io("HID handle mutex poisoned".into()))?;
            unsafe {
                let ok = WriteFile(
                    handle,
                    raw.as_ptr(),
                    raw.len() as u32,
                    &mut written,
                    &mut overlapped,
                );
                if ok == 0 {
                    let err = GetLastError();
                    if err != ERROR_IO_PENDING {
                        return Err(last_error_code("WriteFile", err));
                    }
                    let wait = WaitForSingleObject(event, duration_to_ms(HID_WRITE_TIMEOUT));
                    if wait == WAIT_TIMEOUT {
                        CancelIoEx(handle, &mut overlapped);
                        let mut cancelled = 0u32;
                        let _ = GetOverlappedResult(handle, &mut overlapped, &mut cancelled, 1);
                        return Err(TransportError::Timeout);
                    }
                    if wait != WAIT_OBJECT_0 {
                        CancelIoEx(handle, &mut overlapped);
                        return Err(last_error("WaitForSingleObject"));
                    }
                    let ok = GetOverlappedResult(handle, &mut overlapped, &mut written, 0);
                    if ok == 0 {
                        return Err(last_error("GetOverlappedResult"));
                    }
                }
            }
            if written != raw.len() as u32 {
                return Err(TransportError::Io(format!(
                    "short HID write: {written}/{}",
                    raw.len()
                )));
            }
            Ok(())
        })();
        unsafe {
            CloseHandle(event);
        }
        result
    }

    fn recv(&self) -> Result<HidMessage, TransportError> {
        self.recv_timeout(Duration::from_millis(u32::MAX as u64))
            .and_then(|msg| msg.ok_or(TransportError::Timeout))
    }

    fn is_connected(&self) -> bool {
        self.handle
            .lock()
            .map(|handle| !handle.is_null() && *handle != INVALID_HANDLE_VALUE)
            .unwrap_or(false)
    }
}

pub fn resolve_win_hid_device() -> Result<Option<String>, TransportError> {
    if let Ok(path) = std::env::var("VIBE_WINHID_DEVICE") {
        if !path.trim().is_empty() {
            return Ok(Some(path));
        }
    }
    Ok(list_win_hid_devices()?
        .into_iter()
        .find(|dev| dev.vid == Some(VIBE_USB_VID) && dev.pid == Some(VIBE_USB_PID))
        .map(|dev| dev.path))
}

fn open_device(device: &str) -> Result<WinHidTransport, TransportError> {
    if device == "auto" {
        WinHidTransport::open_auto()
    } else {
        WinHidTransport::open(device)
    }
}

pub fn list_win_hid_devices() -> Result<Vec<WinHidDeviceInfo>, TransportError> {
    unsafe {
        let mut guid = zeroed();
        HidD_GetHidGuid(&mut guid);
        let info = SetupDiGetClassDevsW(
            &guid,
            null(),
            null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        );
        if info == -1 {
            return Err(last_error("SetupDiGetClassDevsW"));
        }
        let mut out = Vec::new();
        let mut index = 0;
        loop {
            let mut iface: SP_DEVICE_INTERFACE_DATA = zeroed();
            iface.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
            let ok = SetupDiEnumDeviceInterfaces(info, null_mut(), &guid, index, &mut iface);
            if ok == 0 {
                break;
            }
            let mut required = 0u32;
            let _ = SetupDiGetDeviceInterfaceDetailW(
                info,
                &mut iface,
                null_mut(),
                0,
                &mut required,
                null_mut(),
            );
            if required > 0 {
                let mut buf = vec![0u8; required as usize];
                let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
                (*detail).cbSize = if size_of::<usize>() == 8 { 8 } else { 6 };
                let ok = SetupDiGetDeviceInterfaceDetailW(
                    info,
                    &mut iface,
                    detail,
                    required,
                    &mut required,
                    null_mut(),
                );
                if ok != 0 {
                    let path = wide_ptr_to_string((*detail).DevicePath.as_ptr());
                    let (vid, pid) = extract_vid_pid(&path);
                    out.push(WinHidDeviceInfo { path, vid, pid });
                }
            }
            index += 1;
        }
        SetupDiDestroyDeviceInfoList(info);
        Ok(out)
    }
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn wide_ptr_to_string(ptr: *const u16) -> String {
    let mut len = 0usize;
    while *ptr.add(len) != 0 {
        len += 1;
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    String::from_utf16_lossy(slice)
}

fn extract_vid_pid(path: &str) -> (Option<u16>, Option<u16>) {
    let lower = path.to_ascii_lowercase();
    (
        extract_hex_after(&lower, "vid_"),
        extract_hex_after(&lower, "pid_"),
    )
}

fn extract_hex_after(text: &str, marker: &str) -> Option<u16> {
    let idx = text.find(marker)?;
    let value = text.get(idx + marker.len()..idx + marker.len() + 4)?;
    u16::from_str_radix(value, 16).ok()
}

fn duration_to_ms(timeout: Duration) -> u32 {
    let ms = timeout.as_millis();
    if ms > u32::MAX as u128 {
        u32::MAX
    } else {
        ms as u32
    }
}

fn last_error(context: &str) -> TransportError {
    unsafe { last_error_code(context, GetLastError()) }
}

fn last_error_code(context: &str, code: u32) -> TransportError {
    TransportError::Io(format!("{context} failed with Win32 error {code}"))
}
