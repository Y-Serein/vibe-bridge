"""Native Windows HID transport.

This keeps the wire format identical to Linux ``HidrawTransport``: one
``hid_protocol.Packet`` per 64-byte vendor HID report, padded with zeros on
write.  Device discovery uses SetupAPI and filters by VID/PID.
"""

from __future__ import annotations

import ctypes
import ctypes.wintypes as wt
import os
import platform
import threading
from dataclasses import dataclass
from typing import List, Optional

from .hid_protocol import HEADER_SIZE, Packet, ProtocolError
from .transport import Transport, TransportClosed
from .transport_hidraw import DEFAULT_REPORT_LENGTH

VIBE_USB_VID = 0x359F
VIBE_USB_PID = 0x2120

DIGCF_PRESENT = 0x00000002
DIGCF_DEVICEINTERFACE = 0x00000010
INVALID_HANDLE_VALUE = wt.HANDLE(-1).value
GENERIC_READ = 0x80000000
GENERIC_WRITE = 0x40000000
FILE_SHARE_READ = 0x00000001
FILE_SHARE_WRITE = 0x00000002
OPEN_EXISTING = 3
FILE_FLAG_OVERLAPPED = 0x40000000
ERROR_IO_PENDING = 997
ERROR_OPERATION_ABORTED = 995
WAIT_OBJECT_0 = 0x00000000
WAIT_TIMEOUT = 0x00000102
INFINITE = 0xFFFFFFFF


@dataclass
class WinHidDeviceInfo:
    path: str
    vid: Optional[int] = None
    pid: Optional[int] = None
    readable: bool = False
    writable: bool = False
    err: Optional[str] = None

    def vid_pid_str(self) -> str:
        if self.vid is None or self.pid is None:
            return "??:??"
        return f"{self.vid:04x}:{self.pid:04x}"


class GUID(ctypes.Structure):
    _fields_ = [
        ("Data1", wt.DWORD),
        ("Data2", wt.WORD),
        ("Data3", wt.WORD),
        ("Data4", wt.BYTE * 8),
    ]


class SP_DEVICE_INTERFACE_DATA(ctypes.Structure):
    _fields_ = [
        ("cbSize", wt.DWORD),
        ("InterfaceClassGuid", GUID),
        ("Flags", wt.DWORD),
        ("Reserved", ctypes.c_void_p),
    ]


class OVERLAPPED(ctypes.Structure):
    _fields_ = [
        ("Internal", ctypes.c_void_p),
        ("InternalHigh", ctypes.c_void_p),
        ("Offset", wt.DWORD),
        ("OffsetHigh", wt.DWORD),
        ("hEvent", wt.HANDLE),
    ]


def _require_windows() -> None:
    if platform.system().lower() != "windows":
        raise OSError("Windows HID transport is only available on native Windows")


def _libs():
    _require_windows()
    hid = ctypes.WinDLL("hid", use_last_error=True)
    setupapi = ctypes.WinDLL("setupapi", use_last_error=True)
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

    hid.HidD_GetHidGuid.argtypes = [ctypes.POINTER(GUID)]
    hid.HidD_GetHidGuid.restype = None

    setupapi.SetupDiGetClassDevsW.argtypes = [
        ctypes.POINTER(GUID),
        wt.LPCWSTR,
        wt.HWND,
        wt.DWORD,
    ]
    setupapi.SetupDiGetClassDevsW.restype = wt.HANDLE
    setupapi.SetupDiEnumDeviceInterfaces.argtypes = [
        wt.HANDLE,
        ctypes.c_void_p,
        ctypes.POINTER(GUID),
        wt.DWORD,
        ctypes.POINTER(SP_DEVICE_INTERFACE_DATA),
    ]
    setupapi.SetupDiEnumDeviceInterfaces.restype = wt.BOOL
    setupapi.SetupDiGetDeviceInterfaceDetailW.argtypes = [
        wt.HANDLE,
        ctypes.POINTER(SP_DEVICE_INTERFACE_DATA),
        ctypes.c_void_p,
        wt.DWORD,
        ctypes.POINTER(wt.DWORD),
        ctypes.c_void_p,
    ]
    setupapi.SetupDiGetDeviceInterfaceDetailW.restype = wt.BOOL
    setupapi.SetupDiDestroyDeviceInfoList.argtypes = [wt.HANDLE]
    setupapi.SetupDiDestroyDeviceInfoList.restype = wt.BOOL

    kernel32.CreateFileW.argtypes = [
        wt.LPCWSTR,
        wt.DWORD,
        wt.DWORD,
        ctypes.c_void_p,
        wt.DWORD,
        wt.DWORD,
        wt.HANDLE,
    ]
    kernel32.CreateFileW.restype = wt.HANDLE
    kernel32.ReadFile.argtypes = [
        wt.HANDLE,
        ctypes.c_void_p,
        wt.DWORD,
        ctypes.POINTER(wt.DWORD),
        ctypes.c_void_p,
    ]
    kernel32.ReadFile.restype = wt.BOOL
    kernel32.WriteFile.argtypes = [
        wt.HANDLE,
        ctypes.c_void_p,
        wt.DWORD,
        ctypes.POINTER(wt.DWORD),
        ctypes.c_void_p,
    ]
    kernel32.WriteFile.restype = wt.BOOL
    kernel32.GetOverlappedResult.argtypes = [
        wt.HANDLE,
        ctypes.POINTER(OVERLAPPED),
        ctypes.POINTER(wt.DWORD),
        wt.BOOL,
    ]
    kernel32.GetOverlappedResult.restype = wt.BOOL
    kernel32.CancelIoEx.argtypes = [wt.HANDLE, ctypes.POINTER(OVERLAPPED)]
    kernel32.CancelIoEx.restype = wt.BOOL
    kernel32.CreateEventW.argtypes = [
        ctypes.c_void_p,
        wt.BOOL,
        wt.BOOL,
        wt.LPCWSTR,
    ]
    kernel32.CreateEventW.restype = wt.HANDLE
    kernel32.WaitForSingleObject.argtypes = [wt.HANDLE, wt.DWORD]
    kernel32.WaitForSingleObject.restype = wt.DWORD
    kernel32.CloseHandle.argtypes = [wt.HANDLE]
    kernel32.CloseHandle.restype = wt.BOOL
    kernel32.FormatMessageW.argtypes = [
        wt.DWORD,
        ctypes.c_void_p,
        wt.DWORD,
        wt.DWORD,
        wt.LPWSTR,
        wt.DWORD,
        ctypes.c_void_p,
    ]
    kernel32.FormatMessageW.restype = wt.DWORD

    return hid, setupapi, kernel32


def _last_error_message(kernel32) -> str:
    err = ctypes.get_last_error()
    buf = ctypes.create_unicode_buffer(512)
    kernel32.FormatMessageW(
        0x00001000 | 0x00000200,
        None,
        err,
        0,
        buf,
        len(buf),
        None,
    )
    msg = buf.value.strip()
    return msg or f"Win32 error {err}"


def _extract_vid_pid(path: str) -> tuple[Optional[int], Optional[int]]:
    lower = path.lower()
    vid = pid = None
    for marker, attr in (("vid_", "vid"), ("pid_", "pid")):
        idx = lower.find(marker)
        if idx < 0:
            continue
        value = lower[idx + len(marker) : idx + len(marker) + 4]
        try:
            parsed = int(value, 16)
        except ValueError:
            continue
        if attr == "vid":
            vid = parsed
        else:
            pid = parsed
    return vid, pid


def list_win_hid_devices(
    *, vid: int = VIBE_USB_VID, pid: int = VIBE_USB_PID
) -> List[WinHidDeviceInfo]:
    """Enumerate present Windows HID interfaces matching ``vid:pid``."""
    hid, setupapi, kernel32 = _libs()

    hid_guid = GUID()
    hid.HidD_GetHidGuid(ctypes.byref(hid_guid))

    dev_info = setupapi.SetupDiGetClassDevsW(
        ctypes.byref(hid_guid),
        None,
        None,
        DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
    )
    if dev_info == INVALID_HANDLE_VALUE:
        raise OSError(_last_error_message(kernel32))

    out: List[WinHidDeviceInfo] = []
    try:
        index = 0
        while True:
            iface = SP_DEVICE_INTERFACE_DATA()
            iface.cbSize = ctypes.sizeof(SP_DEVICE_INTERFACE_DATA)
            ok = setupapi.SetupDiEnumDeviceInterfaces(
                dev_info, None, ctypes.byref(hid_guid), index, ctypes.byref(iface)
            )
            if not ok:
                break

            required = wt.DWORD(0)
            setupapi.SetupDiGetDeviceInterfaceDetailW(
                dev_info, ctypes.byref(iface), None, 0, ctypes.byref(required), None
            )
            detail = ctypes.create_string_buffer(required.value)
            ctypes.cast(detail, ctypes.POINTER(wt.DWORD))[0] = (
                8 if ctypes.sizeof(ctypes.c_void_p) == 8 else 6
            )
            ok = setupapi.SetupDiGetDeviceInterfaceDetailW(
                dev_info,
                ctypes.byref(iface),
                ctypes.cast(detail, ctypes.c_void_p),
                required.value,
                ctypes.byref(required),
                None,
            )
            if ok:
                path = ctypes.wstring_at(ctypes.addressof(detail) + ctypes.sizeof(wt.DWORD))
                found_vid, found_pid = _extract_vid_pid(path)
                if found_vid == vid and found_pid == pid:
                    readable = writable = False
                    err = None
                    try:
                        probe = WinHidTransport(path)
                        probe.close()
                        readable = writable = True
                    except OSError as exc:
                        err = str(exc)
                    out.append(
                        WinHidDeviceInfo(
                            path=path,
                            vid=found_vid,
                            pid=found_pid,
                            readable=readable,
                            writable=writable,
                            err=err,
                        )
                    )
            index += 1
    finally:
        setupapi.SetupDiDestroyDeviceInfoList(dev_info)
    return out


def resolve_win_hid_device() -> Optional[str]:
    explicit = os.environ.get("VIBE_WINHID_DEVICE")
    if explicit:
        return explicit
    devices = list_win_hid_devices()
    if not devices:
        return None
    rw = [d for d in devices if d.readable and d.writable]
    return (rw[0] if rw else devices[0]).path


class WinHidTransport(Transport):
    """Transport backed by a native Windows HID device path."""

    def __init__(self, device_path: str, *, report_length: int = DEFAULT_REPORT_LENGTH) -> None:
        _require_windows()
        self._device_path = device_path
        self._report_length = report_length
        self._lock = threading.Lock()
        _, _, kernel32 = _libs()
        self._kernel32 = kernel32
        self._handle = kernel32.CreateFileW(
            device_path,
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            None,
        )
        if self._handle == INVALID_HANDLE_VALUE:
            raise OSError(_last_error_message(kernel32))

    @property
    def device_path(self) -> str:
        return self._device_path

    @property
    def report_length(self) -> int:
        return self._report_length

    def send_packet(self, packet: Packet) -> None:
        raw = packet.encode()
        if len(raw) > self._report_length:
            raise ProtocolError(
                f"packet too large for HID report: {len(raw)} > {self._report_length}"
            )
        raw = raw + b"\x00" * (self._report_length - len(raw))
        written = wt.DWORD(0)
        buf = ctypes.create_string_buffer(raw)
        event = self._kernel32.CreateEventW(None, True, False, None)
        if event == INVALID_HANDLE_VALUE:
            raise OSError(_last_error_message(self._kernel32))
        overlapped = OVERLAPPED()
        overlapped.hEvent = event
        with self._lock:
            try:
                ok = self._kernel32.WriteFile(
                    self._handle,
                    buf,
                    len(raw),
                    ctypes.byref(written),
                    ctypes.byref(overlapped),
                )
                if not ok:
                    err = ctypes.get_last_error()
                    if err != ERROR_IO_PENDING:
                        raise TransportClosed(_last_error_message(self._kernel32))
                    wait = self._kernel32.WaitForSingleObject(event, INFINITE)
                    if wait != WAIT_OBJECT_0:
                        self._kernel32.CancelIoEx(self._handle, ctypes.byref(overlapped))
                        raise TransportClosed(_last_error_message(self._kernel32))
                    ok = self._kernel32.GetOverlappedResult(
                        self._handle, ctypes.byref(overlapped), ctypes.byref(written), False
                    )
                    if not ok:
                        err = ctypes.get_last_error()
                        if err == ERROR_OPERATION_ABORTED:
                            raise TransportClosed("Windows HID write aborted")
                        raise TransportClosed(_last_error_message(self._kernel32))
                if written.value != len(raw):
                    raise OSError(f"short Windows HID write: {written.value}/{len(raw)}")
            finally:
                self._kernel32.CloseHandle(event)

    def _timeout_ms(self, timeout: Optional[float]) -> int:
        if timeout is None:
            return INFINITE
        if timeout <= 0:
            return 0
        return max(1, int(timeout * 1000))

    def recv_report(self, timeout: Optional[float] = None) -> Optional[bytes]:
        read = wt.DWORD(0)
        buf = ctypes.create_string_buffer(self._report_length)
        event = self._kernel32.CreateEventW(None, True, False, None)
        if event == INVALID_HANDLE_VALUE:
            raise OSError(_last_error_message(self._kernel32))
        overlapped = OVERLAPPED()
        overlapped.hEvent = event
        try:
            ok = self._kernel32.ReadFile(
                self._handle,
                buf,
                self._report_length,
                ctypes.byref(read),
                ctypes.byref(overlapped),
            )
            if not ok:
                err = ctypes.get_last_error()
                if err != ERROR_IO_PENDING:
                    if err == ERROR_OPERATION_ABORTED:
                        raise TransportClosed("Windows HID read aborted")
                    raise TransportClosed(_last_error_message(self._kernel32))

                wait = self._kernel32.WaitForSingleObject(event, self._timeout_ms(timeout))
                if wait == WAIT_TIMEOUT:
                    self._kernel32.CancelIoEx(self._handle, ctypes.byref(overlapped))
                    self._kernel32.GetOverlappedResult(
                        self._handle, ctypes.byref(overlapped), ctypes.byref(read), True
                    )
                    return None
                if wait != WAIT_OBJECT_0:
                    self._kernel32.CancelIoEx(self._handle, ctypes.byref(overlapped))
                    raise TransportClosed(_last_error_message(self._kernel32))
                ok = self._kernel32.GetOverlappedResult(
                    self._handle, ctypes.byref(overlapped), ctypes.byref(read), False
                )
                if not ok:
                    err = ctypes.get_last_error()
                    if err == ERROR_OPERATION_ABORTED:
                        raise TransportClosed("Windows HID read aborted")
                    raise TransportClosed(_last_error_message(self._kernel32))
            if read.value == 0:
                raise TransportClosed("Windows HID returned EOF")
            return bytes(buf.raw[: read.value])
        finally:
            self._kernel32.CloseHandle(event)

    def recv_packet(self, timeout: Optional[float] = None) -> Optional[Packet]:
        data = self.recv_report(timeout)
        if data is None:
            return None
        if len(data) < HEADER_SIZE:
            raise ProtocolError(
                f"Windows HID read shorter than packet header: {len(data)} < {HEADER_SIZE}"
            )
        return Packet.decode(data)

    def close(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is None or handle == INVALID_HANDLE_VALUE:
            return
        try:
            self._kernel32.CloseHandle(handle)
        finally:
            self._handle = None
