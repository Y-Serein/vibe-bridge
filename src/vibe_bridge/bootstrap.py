"""Daemon bootstrap helpers.

Used by the shell wrappers (bin/codex, bin/claude, ...) to make sure a daemon
is reachable before they try to acquire a session id. If the daemon is not
running, ``ensure_daemon_running`` spawns a new detached one and waits up to
``timeout`` seconds for the socket to come alive.
"""

from __future__ import annotations

import json
import os
import socket
import subprocess
import sys
import time
from typing import Optional

from .mock_hid import DEFAULT_SOCK_PATH

ENV_HIDRAW_DEVICE = "VIBE_HIDRAW_DEVICE"
VIBE_USB_VID = 0x359F
VIBE_USB_PID = 0x2120
DEFAULT_STATE_PATH = "/tmp/vibe-bridge-state.json"


def can_connect(sock_path: str, *, timeout: float = 0.2) -> bool:
    """Return True if a Unix socket at ``sock_path`` accepts a connection."""
    if not os.path.exists(sock_path):
        return False
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    try:
        s.connect(sock_path)
        return True
    except (OSError, socket.timeout):
        return False
    finally:
        s.close()


def spawn_daemon_detached(
    sock_path: str = DEFAULT_SOCK_PATH,
    *,
    log_path: str = "/tmp/vibe-bridge-daemon.log",
    extra_args: Optional[list] = None,
) -> int:
    """Start a daemon detached from this process; return its pid.

    The daemon's stdout/stderr are appended to ``log_path``. The child has its
    own session (``start_new_session=True``) so it survives the wrapper's
    ``execvp`` into the real CLI.
    """
    args = [sys.executable, "-m", "vibe_bridge.main", "--sock", sock_path, "daemon"]
    if extra_args:
        args.extend(extra_args)
    log_fd = open(log_path, "ab")
    try:
        proc = subprocess.Popen(
            args,
            stdin=subprocess.DEVNULL,
            stdout=log_fd,
            stderr=log_fd,
            start_new_session=True,
            cwd="/",
            env=_env_for_subprocess(),
        )
    finally:
        log_fd.close()
    return proc.pid


def _env_for_subprocess() -> dict:
    """Pass through the parent env but ensure PYTHONPATH points at our src/."""
    env = os.environ.copy()
    here = os.path.dirname(os.path.abspath(__file__))  # .../src/vibe_bridge
    src_root = os.path.dirname(here)  # .../src
    existing = env.get("PYTHONPATH", "")
    parts = [p for p in existing.split(os.pathsep) if p]
    if src_root not in parts:
        parts.insert(0, src_root)
    env["PYTHONPATH"] = os.pathsep.join(parts)
    return env


def resolve_hidraw_device() -> Optional[str]:
    """Return the hidraw node to use for the board, if one is discoverable."""
    explicit = os.environ.get(ENV_HIDRAW_DEVICE)
    if explicit:
        return explicit

    try:
        from .transport_hidraw import list_hidraw_devices
    except Exception:
        return None

    try:
        devices = list_hidraw_devices()
    except Exception:
        return None

    exact = [d for d in devices if d.vid == VIBE_USB_VID and d.pid == VIBE_USB_PID]
    if exact:
        rw = [d for d in exact if d.readable and d.writable]
        return (rw[0] if rw else exact[0]).path

    rw_hidraw = [d for d in devices if d.readable and d.writable]
    if len(rw_hidraw) == 1:
        return rw_hidraw[0].path
    return None


def _load_daemon_state(state_path: str) -> Optional[dict]:
    try:
        with open(state_path, "r", encoding="utf-8") as f:
            state = json.load(f)
    except (OSError, json.JSONDecodeError):
        return None
    return state if isinstance(state, dict) else None


def _state_matches_hidraw_daemon(
    sock_path: str, hidraw_device: str, state_path: str
) -> bool:
    state = _load_daemon_state(state_path)
    if state is None:
        return False
    if state.get("sock_path") != sock_path:
        return False
    if state.get("mode") != "real-hidraw":
        return False
    state_hidraw = state.get("hidraw_path")
    if not isinstance(state_hidraw, str) or not state_hidraw:
        return False
    return os.path.realpath(state_hidraw) == os.path.realpath(hidraw_device)


def _daemon_ready(sock_path: str, hidraw_device: Optional[str], state_path: str) -> bool:
    if not can_connect(sock_path):
        return False
    if hidraw_device is None:
        return True
    if sock_path != DEFAULT_SOCK_PATH:
        return True
    return _state_matches_hidraw_daemon(sock_path, hidraw_device, state_path)


def ensure_daemon_running(
    sock_path: str = DEFAULT_SOCK_PATH,
    *,
    state_path: str = DEFAULT_STATE_PATH,
    timeout: float = 3.0,
    poll_interval: float = 0.1,
    log_path: str = "/tmp/vibe-bridge-daemon.log",
) -> bool:
    """Return True if a daemon is reachable, spawning one if necessary.

    Blocks up to ``timeout`` seconds while the freshly-spawned daemon binds.
    Returns False if no daemon could be reached in time; callers should fall
    back to running their CLI without a session.
    """
    hidraw_device = resolve_hidraw_device()
    if _daemon_ready(sock_path, hidraw_device, state_path):
        return True

    extra_args = ["--state", state_path]
    if hidraw_device:
        extra_args.extend(["--hidraw", hidraw_device])
    spawn_daemon_detached(sock_path, log_path=log_path, extra_args=extra_args)
    deadline = time.time() + timeout
    while time.time() < deadline:
        if _daemon_ready(sock_path, hidraw_device, state_path):
            return True
        time.sleep(poll_interval)
    return False
