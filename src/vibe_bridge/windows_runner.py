"""Windows ConPTY runner for product CLI adapters."""

from __future__ import annotations

import ctypes
import ctypes.wintypes as wt
import os
import platform
import queue
import shutil
import subprocess
import sys
import threading
from typing import Callable, Optional, Sequence

from .forwarder import Forwarder
from .hid_protocol import Cmd, Packet, ReportId
from .plugin_client import PluginClient, PluginError
from .mock_hid import DEFAULT_TCP_ENDPOINT

ChunkCallback = Callable[[bytes], None]

COORD_ERROR = 0x80070057
EXTENDED_STARTUPINFO_PRESENT = 0x00080000
CREATE_UNICODE_ENVIRONMENT = 0x00000400
PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE = 0x00020016
GENERIC_READ = 0x80000000
GENERIC_WRITE = 0x40000000
WAIT_OBJECT_0 = 0x00000000
WAIT_TIMEOUT = 0x00000102
INFINITE = 0xFFFFFFFF
CTRL_C_EXIT_CODE = 130


class COORD(ctypes.Structure):
    _fields_ = [("X", ctypes.c_short), ("Y", ctypes.c_short)]


class STARTUPINFOW(ctypes.Structure):
    _fields_ = [
        ("cb", wt.DWORD),
        ("lpReserved", wt.LPWSTR),
        ("lpDesktop", wt.LPWSTR),
        ("lpTitle", wt.LPWSTR),
        ("dwX", wt.DWORD),
        ("dwY", wt.DWORD),
        ("dwXSize", wt.DWORD),
        ("dwYSize", wt.DWORD),
        ("dwXCountChars", wt.DWORD),
        ("dwYCountChars", wt.DWORD),
        ("dwFillAttribute", wt.DWORD),
        ("dwFlags", wt.DWORD),
        ("wShowWindow", wt.WORD),
        ("cbReserved2", wt.WORD),
        ("lpReserved2", ctypes.c_void_p),
        ("hStdInput", wt.HANDLE),
        ("hStdOutput", wt.HANDLE),
        ("hStdError", wt.HANDLE),
    ]


class STARTUPINFOEXW(ctypes.Structure):
    _fields_ = [
        ("StartupInfo", STARTUPINFOW),
        ("lpAttributeList", ctypes.c_void_p),
    ]


class PROCESS_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("hProcess", wt.HANDLE),
        ("hThread", wt.HANDLE),
        ("dwProcessId", wt.DWORD),
        ("dwThreadId", wt.DWORD),
    ]


def _require_windows() -> None:
    if platform.system().lower() != "windows":
        raise OSError("windows run requires native Windows")


def _kernel32():
    _require_windows()
    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    k32.CreatePipe.argtypes = [
        ctypes.POINTER(wt.HANDLE),
        ctypes.POINTER(wt.HANDLE),
        ctypes.c_void_p,
        wt.DWORD,
    ]
    k32.CreatePipe.restype = wt.BOOL
    k32.CreatePseudoConsole.argtypes = [
        COORD,
        wt.HANDLE,
        wt.HANDLE,
        wt.DWORD,
        ctypes.POINTER(ctypes.c_void_p),
    ]
    k32.CreatePseudoConsole.restype = ctypes.c_long
    k32.ClosePseudoConsole.argtypes = [ctypes.c_void_p]
    k32.ClosePseudoConsole.restype = None
    k32.InitializeProcThreadAttributeList.argtypes = [
        ctypes.c_void_p,
        wt.DWORD,
        wt.DWORD,
        ctypes.POINTER(ctypes.c_size_t),
    ]
    k32.InitializeProcThreadAttributeList.restype = wt.BOOL
    k32.UpdateProcThreadAttribute.argtypes = [
        ctypes.c_void_p,
        wt.DWORD,
        ctypes.c_size_t,
        ctypes.c_void_p,
        ctypes.c_size_t,
        ctypes.c_void_p,
        ctypes.c_void_p,
    ]
    k32.UpdateProcThreadAttribute.restype = wt.BOOL
    k32.DeleteProcThreadAttributeList.argtypes = [ctypes.c_void_p]
    k32.DeleteProcThreadAttributeList.restype = None
    k32.CreateProcessW.argtypes = [
        wt.LPCWSTR,
        wt.LPWSTR,
        ctypes.c_void_p,
        ctypes.c_void_p,
        wt.BOOL,
        wt.DWORD,
        wt.LPCVOID,
        wt.LPCWSTR,
        ctypes.POINTER(STARTUPINFOEXW),
        ctypes.POINTER(PROCESS_INFORMATION),
    ]
    k32.CreateProcessW.restype = wt.BOOL
    k32.ReadFile.argtypes = [
        wt.HANDLE,
        ctypes.c_void_p,
        wt.DWORD,
        ctypes.POINTER(wt.DWORD),
        ctypes.c_void_p,
    ]
    k32.ReadFile.restype = wt.BOOL
    k32.WriteFile.argtypes = [
        wt.HANDLE,
        ctypes.c_void_p,
        wt.DWORD,
        ctypes.POINTER(wt.DWORD),
        ctypes.c_void_p,
    ]
    k32.WriteFile.restype = wt.BOOL
    k32.CloseHandle.argtypes = [wt.HANDLE]
    k32.CloseHandle.restype = wt.BOOL
    k32.WaitForSingleObject.argtypes = [wt.HANDLE, wt.DWORD]
    k32.WaitForSingleObject.restype = wt.DWORD
    k32.GetExitCodeProcess.argtypes = [wt.HANDLE, ctypes.POINTER(wt.DWORD)]
    k32.GetExitCodeProcess.restype = wt.BOOL
    k32.TerminateProcess.argtypes = [wt.HANDLE, wt.UINT]
    k32.TerminateProcess.restype = wt.BOOL
    k32.GetLastError.restype = wt.DWORD
    return k32


def _last_error_message(k32) -> str:
    return ctypes.FormatError(ctypes.get_last_error() or k32.GetLastError())


def _command_line(argv: Sequence[str]) -> str:
    if not argv:
        raise ValueError("argv must not be empty")
    return subprocess.list2cmdline(list(argv))


def build_wsl_command(
    argv: Sequence[str],
    *,
    distro: Optional[str] = None,
    cwd: Optional[str] = None,
) -> list[str]:
    """Build the Windows-host command for a WSL-backed CLI session."""
    command = list(argv)
    if command and command[0] == "--":
        command = command[1:]
    out = ["wsl.exe"]
    if distro:
        out.extend(["-d", distro])
    if cwd:
        out.extend(["--cd", cwd])
    if command:
        out.append("--")
        out.extend(command)
    return out


def _env_block(env: dict) -> str:
    items = []
    for key, value in env.items():
        if key and "=" not in key:
            items.append((str(key), str(value)))
    return "".join(f"{k}={v}\0" for k, v in sorted(items, key=lambda kv: kv[0].upper())) + "\0"


def _console_size(default_cols: int = 80, default_rows: int = 24) -> COORD:
    try:
        size = os.get_terminal_size()
        return COORD(max(1, size.columns), max(1, size.lines))
    except OSError:
        return COORD(default_cols, default_rows)


def _create_pipe(k32) -> tuple[int, int]:
    read = wt.HANDLE()
    write = wt.HANDLE()
    if not k32.CreatePipe(ctypes.byref(read), ctypes.byref(write), None, 0):
        raise OSError(_last_error_message(k32))
    return int(read.value), int(write.value)


def _write_handle(k32, handle: int, data: bytes) -> bool:
    if not data:
        return True
    written = wt.DWORD(0)
    buf = ctypes.create_string_buffer(data)
    ok = k32.WriteFile(handle, buf, len(data), ctypes.byref(written), None)
    return bool(ok and written.value == len(data))


def _close_handle(k32, handle: Optional[int]) -> None:
    if handle:
        try:
            k32.CloseHandle(handle)
        except Exception:
            pass


def _activate_session(plugin: PluginClient, sid: int) -> None:
    plugin.send_packet(
        Packet(
            report_id=int(ReportId.HOST_BOUND),
            command=int(Cmd.WINDOW_ACTIVATE),
            session_id=sid,
            payload=b"",
        )
    )


def run_windows_cli(
    argv: Sequence[str],
    *,
    sock_path: str = DEFAULT_TCP_ENDPOINT,
    plugin_name: Optional[str] = None,
    env: Optional[dict] = None,
    on_output: Optional[ChunkCallback] = None,
) -> int:
    """Run ``argv`` under ConPTY and mirror output to the bridge daemon."""
    argv = list(argv)
    if argv and argv[0] == "--":
        argv = argv[1:]
    if not argv:
        print("windows run: missing command", file=sys.stderr)
        return 2
    _require_windows()

    real = shutil.which(argv[0])
    if real is None:
        print(f"windows run: command not found: {argv[0]}", file=sys.stderr)
        return 127
    child_argv = [real] + argv[1:]
    plugin_label = plugin_name or os.path.basename(real)

    plugin = PluginClient(
        plugin_name=plugin_label,
        sock_path=sock_path,
        auto_reacquire=False,
    )
    try:
        plugin.connect()
        sid = plugin.acquire_session(timeout=3.0)
    except (OSError, PluginError) as exc:
        plugin.close()
        print(f"windows run: bridge session failed: {exc}", file=sys.stderr)
        return 1

    forwarder = Forwarder(plugin.send_vt100)
    forwarder.start()
    injected_input: "queue.Queue[bytes]" = queue.Queue()
    try:
        from .wrapper import BoardActionHandler, LcdOutputAdapter

        adapter = LcdOutputAdapter(theme=os.environ.get("VIBE_BRIDGE_LCD_THEME", ""))
        action_handler = BoardActionHandler(plugin=plugin, inject_input=injected_input.put)
        plugin.set_board_packet_handler(action_handler.handle_packet)
    except Exception:
        adapter = None

    def forward(chunk: bytes) -> None:
        if on_output is not None:
            on_output(chunk)
        forwarder.push(adapter.feed(chunk) if adapter is not None else chunk)

    child_env = dict(os.environ if env is None else env)
    child_env["VIBE_SOCK_PATH"] = sock_path
    child_env["VIBE_SESSION_ID"] = str(sid)
    child_env["VIBE_BRIDGE_DISABLE"] = "1"
    try:
        return _run_conpty(child_argv, env=child_env, on_output=forward, injected_input=injected_input)
    finally:
        plugin.set_board_packet_handler(None)
        forwarder.stop(timeout=0.5)
        plugin.close()


def run_wsl_cli(
    argv: Sequence[str],
    *,
    sock_path: str = DEFAULT_TCP_ENDPOINT,
    plugin_name: Optional[str] = None,
    distro: Optional[str] = None,
    wsl_cwd: Optional[str] = None,
    env: Optional[dict] = None,
    on_output: Optional[ChunkCallback] = None,
) -> int:
    """Launch ``wsl.exe`` from the native Windows host and mirror it as a session."""
    return run_windows_cli(
        build_wsl_command(argv, distro=distro, cwd=wsl_cwd),
        sock_path=sock_path,
        plugin_name=plugin_name or "wsl-cli",
        env=env,
        on_output=on_output,
    )


def _run_conpty(
    argv: Sequence[str],
    *,
    env: dict,
    on_output: Optional[ChunkCallback],
    injected_input: "queue.Queue[bytes]",
) -> int:
    k32 = _kernel32()
    in_read = in_write = out_read = out_write = None
    hpc = ctypes.c_void_p()
    attr_buf = None
    pi = PROCESS_INFORMATION()
    try:
        in_read, in_write = _create_pipe(k32)
        out_read, out_write = _create_pipe(k32)
        hr = k32.CreatePseudoConsole(_console_size(), in_read, out_write, 0, ctypes.byref(hpc))
        if hr != 0:
            raise OSError(COORD_ERROR, f"CreatePseudoConsole failed: 0x{hr & 0xffffffff:08x}")
        _close_handle(k32, in_read)
        _close_handle(k32, out_write)
        in_read = out_write = None

        attr_size = ctypes.c_size_t()
        k32.InitializeProcThreadAttributeList(None, 1, 0, ctypes.byref(attr_size))
        attr_buf = ctypes.create_string_buffer(attr_size.value)
        attr_ptr = ctypes.cast(attr_buf, ctypes.c_void_p)
        if not k32.InitializeProcThreadAttributeList(attr_ptr, 1, 0, ctypes.byref(attr_size)):
            raise OSError(_last_error_message(k32))
        if not k32.UpdateProcThreadAttribute(
            attr_ptr,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
            hpc,
            ctypes.sizeof(ctypes.c_void_p),
            None,
            None,
        ):
            raise OSError(_last_error_message(k32))

        si = STARTUPINFOEXW()
        si.StartupInfo.cb = ctypes.sizeof(STARTUPINFOEXW)
        si.lpAttributeList = attr_ptr.value
        cmd_text = _command_line(argv)
        cmd = ctypes.create_unicode_buffer(cmd_text)
        env_block = ctypes.create_unicode_buffer(_env_block(env))
        if not k32.CreateProcessW(
            None,
            cmd,
            None,
            None,
            False,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
            env_block,
            None,
            ctypes.byref(si),
            ctypes.byref(pi),
        ):
            raise OSError(f"CreateProcessW failed for {cmd_text!r}: {_last_error_message(k32)}")

        output_thread = threading.Thread(
            target=_output_loop,
            args=(k32, out_read, on_output),
            name="vibe-windows-conpty-output",
            daemon=True,
        )
        input_thread = threading.Thread(
            target=_input_loop,
            args=(k32, in_write),
            name="vibe-windows-conpty-input",
            daemon=True,
        )
        injected_thread = threading.Thread(
            target=_injected_input_loop,
            args=(k32, in_write, injected_input),
            name="vibe-windows-board-input",
            daemon=True,
        )
        output_thread.start()
        input_thread.start()
        injected_thread.start()

        return _wait_for_process(k32, pi.hProcess)
    finally:
        if attr_buf is not None:
            k32.DeleteProcThreadAttributeList(ctypes.cast(attr_buf, ctypes.c_void_p))
        if hpc:
            k32.ClosePseudoConsole(hpc)
        _close_handle(k32, pi.hThread)
        _close_handle(k32, pi.hProcess)
        _close_handle(k32, in_read)
        _close_handle(k32, in_write)
        _close_handle(k32, out_read)
        _close_handle(k32, out_write)


def _wait_for_process(k32, process_handle: int) -> int:
    try:
        while True:
            wait_rc = k32.WaitForSingleObject(process_handle, 100)
            if wait_rc == WAIT_OBJECT_0:
                break
            if wait_rc == WAIT_TIMEOUT:
                continue
            raise OSError(_last_error_message(k32))
    except KeyboardInterrupt:
        try:
            k32.TerminateProcess(process_handle, CTRL_C_EXIT_CODE)
        except Exception:
            pass
        return CTRL_C_EXIT_CODE

    exit_code = wt.DWORD(1)
    k32.GetExitCodeProcess(process_handle, ctypes.byref(exit_code))
    return int(exit_code.value)


def _output_loop(k32, out_read: int, on_output: Optional[ChunkCallback]) -> None:
    while True:
        read = wt.DWORD(0)
        buf = ctypes.create_string_buffer(4096)
        ok = k32.ReadFile(out_read, buf, len(buf), ctypes.byref(read), None)
        if not ok or read.value == 0:
            return
        data = bytes(buf.raw[: read.value])
        try:
            sys.stdout.buffer.write(data)
            sys.stdout.buffer.flush()
        except OSError:
            pass
        if on_output is not None:
            on_output(data)


def _input_loop(k32, in_write: int) -> None:
    while True:
        try:
            data = os.read(sys.stdin.fileno(), 1024)
        except OSError:
            return
        if not data:
            return
        if not _write_handle(k32, in_write, data):
            return


def _injected_input_loop(
    k32,
    in_write: int,
    injected_input: "queue.Queue[bytes]",
) -> None:
    while True:
        data = injected_input.get()
        if not data:
            continue
        if not _write_handle(k32, in_write, data):
            return
