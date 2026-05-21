"""Auto-hook AI agent processes running on the host.

Cross-platform: Windows is the **product host** (``vibe-bridge windows daemon``
with TCP IPC on 127.0.0.1:8765 and ``WinHidTransport``); Linux/WSL is the
**development host** (``vibe-bridge daemon --hidraw …`` with Unix socket IPC).

The scanner thread polls the OS process table once per ``interval`` seconds
and looks for known agent binaries (``codex``, ``claude`` / ``claude-code``).
The first time a matching PID appears it:

1. Opens a fresh plugin connection to the daemon's IPC endpoint
   (``MockHidClient`` accepts ``/path.sock`` *or* ``tcp://host:port``)
   and sends ``CMD_REQUEST_SESSION`` with a hint identifying the agent kind.
2. Waits for ``CMD_SESSION_RESPONSE`` (the board allocated a sid).
3. Sends ``CMD_STATUS_UPDATE(SessionState.RUN)`` so the board's grid row shows
   the session as live immediately, without waiting for the host to forward any
   VT100 bytes.
4. Stores ``pid → (sid, plugin_client, kind)`` in a hook table.

When the PID disappears, the corresponding ``MockHidClient`` is closed; the
daemon sees the IPC disconnect, runs ``_handle_client_disconnect``, and the
board receives ``CMD_SESSION_INVALID(EXPIRED)`` for that sid (so the grid row
clears automatically).

**Stdout capture is intentionally out of scope.** Reading the stdout of an
already-running process from outside requires hacky paths (ptrace / strace /
LD_PRELOAD on Linux, DLL injection on Windows). On Windows the supported way to
attach stdout is to launch the agent via ``vibe-bridge windows cli -- codex``
which sets up ConPTY ownership before the child starts.
"""

from __future__ import annotations

import ctypes
import logging
import os
import sys
import threading
from typing import Dict, List, Optional, Tuple

from .hid_protocol import (
    Cmd,
    SessionState,
    make_request_session,
    make_status_update,
)
from .mock_hid import MockHidClient

log = logging.getLogger("vibe_bridge.agent_scanner")

DEFAULT_SCAN_INTERVAL_SECONDS = 1.0
DEFAULT_PROC_ROOT = "/proc"

IS_WINDOWS = sys.platform == "win32"

# Binary basenames that count as "an agent we want to hook". Keep tight so a
# user editor that happens to mention "claude" in argv (e.g. a docs file path)
# does not get hooked by accident. Windows variants carry .exe suffix.
AGENT_BASENAMES: Dict[str, str] = {
    "codex": "codex",
    "codex.exe": "codex",
    "claude": "claude",
    "claude.exe": "claude",
    "claude-code": "claude",
    "claude-code.exe": "claude",
}

# Cmdline tokens that disqualify the process even if a known basename appears.
# E.g. the scanner's own daemon embeds ``vibe_bridge.main`` so its argv contains
# "claude" only as a forwarded sub-binary path — but that path lives inside a
# wrapper, which sets VIBE_SESSION_ID, so the environ check below catches it.
# (Linux-only; Windows wrappers are not part of the product path.)
WRAPPER_ENV_KEY = b"VIBE_SESSION_ID="


class AgentScanner:
    """Polls /proc for AI agent processes and proxies a session per PID."""

    def __init__(
        self,
        *,
        sock_path: str,
        interval_seconds: float = DEFAULT_SCAN_INTERVAL_SECONDS,
        proc_root: str = DEFAULT_PROC_ROOT,
        agent_basenames: Optional[Dict[str, str]] = None,
        request_timeout: float = 2.0,
    ) -> None:
        self._sock_path = sock_path
        self._interval = max(0.05, float(interval_seconds))
        self._proc_root = proc_root
        self._basenames = dict(agent_basenames or AGENT_BASENAMES)
        self._request_timeout = request_timeout

        self._lock = threading.RLock()
        self._stop = threading.Event()
        self._thread: Optional[threading.Thread] = None
        # pid -> (sid, kind, plugin_client)
        self._hooked: Dict[int, Tuple[int, str, MockHidClient]] = {}

    # ---------------------------------------------------------------- lifecycle

    def start(self) -> None:
        if self._thread is not None:
            return
        self._stop.clear()
        self._thread = threading.Thread(
            target=self._run, name="vibe-bridge-agent-scanner", daemon=True
        )
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        if self._thread is not None:
            self._thread.join(timeout=2.0)
            self._thread = None
        with self._lock:
            for pid, (sid, _kind, client) in list(self._hooked.items()):
                try:
                    client.close()
                except Exception:
                    pass
            self._hooked.clear()

    # ---------------------------------------------------------------- public

    def hook_table(self) -> List[Dict[str, object]]:
        """Snapshot of currently hooked agents, suitable for daemon state dump."""
        with self._lock:
            return [
                {"pid": pid, "kind": kind, "sid": sid}
                for pid, (sid, kind, _client) in sorted(self._hooked.items())
            ]

    # ---------------------------------------------------------------- core loop

    def _run(self) -> None:
        # Fire one scan immediately so the daemon has up-to-date hook state
        # before the first sleep tick.
        try:
            self.scan_once()
        except Exception as exc:
            log.info("agent_scanner: first scan failed: %s", exc)
        while not self._stop.wait(timeout=self._interval):
            try:
                self.scan_once()
            except Exception as exc:
                log.info("agent_scanner: scan failed: %s", exc)

    def scan_once(self) -> None:
        live = self._enumerate_agents()
        with self._lock:
            existing = set(self._hooked)

        for pid, kind in live.items():
            if pid not in existing:
                self._hook(pid, kind)

        for pid in existing - set(live):
            self._unhook(pid)

    # ---------------------------------------------------------------- helpers

    def _enumerate_agents(self) -> Dict[int, str]:
        """Return ``{pid: kind}`` for currently-running agent processes.

        Tests inject a fake ``proc_root`` to exercise the Linux code path even
        on non-Linux test runners; only platform=='win32' triggers the
        Windows ToolHelp path."""
        if IS_WINDOWS and self._proc_root == DEFAULT_PROC_ROOT:
            return self._enumerate_windows()
        return self._enumerate_linux()

    # -- Linux ---------------------------------------------------------------

    def _enumerate_linux(self) -> Dict[int, str]:
        result: Dict[int, str] = {}
        try:
            entries = os.listdir(self._proc_root)
        except OSError as exc:
            log.info("agent_scanner: cannot list %s: %s", self._proc_root, exc)
            return result

        for entry in entries:
            if not entry.isdigit():
                continue
            pid = int(entry)
            kind = self._classify_linux_pid(pid)
            if kind is None:
                continue
            if self._is_wrapper_owned_linux(pid):
                continue
            result[pid] = kind
        return result

    def _classify_linux_pid(self, pid: int) -> Optional[str]:
        cmdline_path = os.path.join(self._proc_root, str(pid), "cmdline")
        try:
            with open(cmdline_path, "rb") as f:
                raw = f.read()
        except OSError:
            return None
        if not raw:
            return None
        for arg in raw.split(b"\x00"):
            if not arg:
                continue
            try:
                base = os.path.basename(arg.decode("utf-8", errors="replace"))
            except Exception:
                continue
            kind = self._basenames.get(base)
            if kind is not None:
                return kind
        return None

    def _is_wrapper_owned_linux(self, pid: int) -> bool:
        """An agent already started through ``~/.local/bin/<name>`` will have
        ``VIBE_SESSION_ID`` in its environ — its wrapper owns the sid and the
        PTY-based stdout transport. Scanner must not double-hook those.
        Windows has no wrapper path so this check is Linux-only."""
        environ_path = os.path.join(self._proc_root, str(pid), "environ")
        try:
            with open(environ_path, "rb") as f:
                env = f.read()
        except OSError:
            return False
        return WRAPPER_ENV_KEY in env

    # -- Windows -------------------------------------------------------------

    def _enumerate_windows(self) -> Dict[int, str]:
        """Use Win32 ToolHelp32 to enumerate processes; match on exe basename.

        ToolHelp gives us ``szExeFile`` (always present) and ``th32ProcessID``.
        Cmdline + env would require ``NtQueryInformationProcess`` + PEB read,
        which is significantly more complex; on Windows the agent binary name
        is enough to classify reliably (``codex.exe`` / ``claude.exe`` etc.)."""
        result: Dict[int, str] = {}
        try:
            entries = _windows_iter_processes()
        except OSError as exc:
            log.info("agent_scanner: Windows process enumeration failed: %s", exc)
            return result
        for pid, exe_name in entries:
            base = os.path.basename(exe_name).lower()
            kind = self._basenames.get(base)
            if kind is not None:
                result[pid] = kind
        return result

    # ---------------------------------------------------------------- hook ops

    def _hook(self, pid: int, kind: str) -> None:
        try:
            client = MockHidClient(self._sock_path)
        except Exception as exc:
            log.info("agent_scanner: cannot connect to %s: %s", self._sock_path, exc)
            return

        try:
            client.send_packet(make_request_session(hint=kind.encode("utf-8")))
        except Exception as exc:
            log.info("agent_scanner: REQUEST_SESSION failed for pid=%d: %s", pid, exc)
            try:
                client.close()
            except Exception:
                pass
            return

        reply = None
        try:
            reply = client.recv_packet(timeout=self._request_timeout)
        except Exception as exc:
            log.info("agent_scanner: recv SESSION_RESPONSE failed for pid=%d: %s", pid, exc)

        if reply is None or reply.command != int(Cmd.SESSION_RESPONSE):
            log.info(
                "agent_scanner: did not get SESSION_RESPONSE for pid=%d (got %r)",
                pid, reply,
            )
            try:
                client.close()
            except Exception:
                pass
            return

        sid = reply.session_id
        # Tell the board this session is live so the grid row appears right
        # away, without depending on host VT100 traffic.
        try:
            client.send_packet(make_status_update(sid, SessionState.RUN))
        except Exception as exc:
            log.info("agent_scanner: STATUS_UPDATE for sid=%d failed: %s", sid, exc)

        with self._lock:
            self._hooked[pid] = (sid, kind, client)
        log.info("agent_scanner: hooked pid=%d kind=%s sid=%d", pid, kind, sid)

    def _unhook(self, pid: int) -> None:
        with self._lock:
            entry = self._hooked.pop(pid, None)
        if entry is None:
            return
        sid, kind, client = entry
        log.info("agent_scanner: unhooking pid=%d kind=%s sid=%d", pid, kind, sid)
        # Closing the plugin socket triggers the daemon's
        # _handle_client_disconnect, which invalidates the sid and notifies
        # the board (CMD_SESSION_INVALID(EXPIRED)).
        try:
            client.close()
        except Exception:
            pass


# ----------------------------------------------------------------- win32 helpers

if IS_WINDOWS:
    # ToolHelp32 process snapshot. Documented in
    # https://learn.microsoft.com/windows/win32/api/tlhelp32/
    from ctypes import wintypes

    _TH32CS_SNAPPROCESS = 0x00000002
    _INVALID_HANDLE_VALUE = ctypes.c_void_p(-1).value

    class _PROCESSENTRY32W(ctypes.Structure):
        _fields_ = [
            ("dwSize", wintypes.DWORD),
            ("cntUsage", wintypes.DWORD),
            ("th32ProcessID", wintypes.DWORD),
            ("th32DefaultHeapID", ctypes.c_void_p),
            ("th32ModuleID", wintypes.DWORD),
            ("cntThreads", wintypes.DWORD),
            ("th32ParentProcessID", wintypes.DWORD),
            ("pcPriClassBase", wintypes.LONG),
            ("dwFlags", wintypes.DWORD),
            ("szExeFile", wintypes.WCHAR * 260),
        ]

    _kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    _kernel32.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
    _kernel32.CreateToolhelp32Snapshot.argtypes = [wintypes.DWORD, wintypes.DWORD]
    _kernel32.Process32FirstW.restype = wintypes.BOOL
    _kernel32.Process32FirstW.argtypes = [wintypes.HANDLE, ctypes.POINTER(_PROCESSENTRY32W)]
    _kernel32.Process32NextW.restype = wintypes.BOOL
    _kernel32.Process32NextW.argtypes = [wintypes.HANDLE, ctypes.POINTER(_PROCESSENTRY32W)]
    _kernel32.CloseHandle.restype = wintypes.BOOL
    _kernel32.CloseHandle.argtypes = [wintypes.HANDLE]


def _windows_iter_processes() -> List[Tuple[int, str]]:
    """Snapshot all processes via ToolHelp32; return list of (pid, exe_name)."""
    if not IS_WINDOWS:
        raise OSError("_windows_iter_processes called on non-Windows host")

    snap = _kernel32.CreateToolhelp32Snapshot(_TH32CS_SNAPPROCESS, 0)
    if snap == _INVALID_HANDLE_VALUE or snap == 0:
        raise OSError(ctypes.get_last_error(),
                      "CreateToolhelp32Snapshot failed")
    try:
        entry = _PROCESSENTRY32W()
        entry.dwSize = ctypes.sizeof(_PROCESSENTRY32W)
        out: List[Tuple[int, str]] = []
        if not _kernel32.Process32FirstW(snap, ctypes.byref(entry)):
            return out
        while True:
            out.append((int(entry.th32ProcessID), str(entry.szExeFile)))
            if not _kernel32.Process32NextW(snap, ctypes.byref(entry)):
                break
        return out
    finally:
        _kernel32.CloseHandle(snap)
