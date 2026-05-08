"""Shell-wrapper entry point.

Used by ``bin/codex``, ``bin/claude`` and friends. The wrapper:

1. Locates the real CLI binary by walking ``$PATH`` and skipping itself.
2. Ensures the vibe-bridge daemon is running (spawns it detached if not).
3. Acquires a session id and stamps it into the environment as
   ``VIBE_SESSION_ID`` and ``VIBE_SOCK_PATH`` for the child to inherit.
4. Either:
   - **PTY mode** (default when stdin & stdout are TTYs): keeps the wrapper
     alive as a parent that owns the PTY master; tees the child's output to
     the user's terminal AND to a forwarder that publishes
     ``CMD_VT100_STREAM`` packets to the daemon.
   - **Exec mode** (non-tty, or ``VIBE_BRIDGE_FORWARD=exec``): ``execvp`` into
     the real CLI for zero overhead. No VT100 forwarding.

If the daemon is unreachable or the session handshake fails, the wrapper logs a
warning and still runs the real CLI so the user is never blocked by the bridge
being broken.
"""

from __future__ import annotations

import os
import sys
from typing import List, Optional

from .bootstrap import can_connect, ensure_daemon_running
from .forwarder import Forwarder
from .mock_hid import DEFAULT_SOCK_PATH
from .plugin_client import PluginClient, PluginError

ENV_SESSION_ID = "VIBE_SESSION_ID"
ENV_SOCK_PATH = "VIBE_SOCK_PATH"
ENV_DISABLE = "VIBE_BRIDGE_DISABLE"
ENV_FORWARD_MODE = "VIBE_BRIDGE_FORWARD"  # "pty" | "exec" | unset (=auto)
ENV_REUSE_SESSION = "VIBE_BRIDGE_REUSE_SESSION"
LEGACY_REAL_SOCK_PATH = "/tmp/vibe-real.sock"


def find_real_binary(name: str, *, exclude_paths: List[str]) -> Optional[str]:
    """Return the first ``name`` on PATH whose realpath is not in ``exclude_paths``."""
    excluded = {os.path.realpath(p) for p in exclude_paths}
    seen = set()
    for raw in os.environ.get("PATH", "").split(os.pathsep):
        d = os.path.expanduser(raw) if raw else "."
        if not d or d in seen:
            continue
        seen.add(d)
        candidate = os.path.join(d, name)
        if not os.access(candidate, os.X_OK):
            continue
        if not os.path.isfile(candidate):
            continue
        if os.path.realpath(candidate) in excluded:
            continue
        return candidate
    return None


def _select_mode() -> str:
    explicit = os.environ.get(ENV_FORWARD_MODE, "").strip().lower()
    if explicit in {"pty", "exec", "none"}:
        return explicit
    if sys.stdin.isatty() and sys.stdout.isatty():
        return "pty"
    return "exec"


def _resolve_sock_path(sock_path: Optional[str]) -> str:
    if sock_path:
        return sock_path
    env_sock = os.environ.get(ENV_SOCK_PATH)
    if env_sock:
        return env_sock
    if can_connect(DEFAULT_SOCK_PATH):
        return DEFAULT_SOCK_PATH
    if can_connect(LEGACY_REAL_SOCK_PATH):
        return LEGACY_REAL_SOCK_PATH
    return DEFAULT_SOCK_PATH


def _existing_session_from_env() -> Optional[int]:
    if os.environ.get(ENV_REUSE_SESSION) != "1":
        return None
    existing = os.environ.get(ENV_SESSION_ID)
    if existing is None:
        return None
    try:
        return int(existing)
    except ValueError:
        return None


def run(
    plugin_name: str,
    *,
    argv: Optional[List[str]] = None,
    sock_path: Optional[str] = None,
    self_paths: Optional[List[str]] = None,
) -> int:
    argv = list(argv if argv is not None else sys.argv)
    sock_path = _resolve_sock_path(sock_path)
    self_paths = list(self_paths or [argv[0] if argv else __file__])

    if os.environ.get(ENV_DISABLE) == "1":
        return _exec_real(plugin_name, argv, self_paths)

    real = find_real_binary(plugin_name, exclude_paths=self_paths)
    if real is None:
        print(
            f"vibe-bridge wrapper: could not find a real `{plugin_name}` on PATH "
            f"(looked through $PATH skipping {self_paths})",
            file=sys.stderr,
        )
        return 127

    session_id = _acquire_session(plugin_name, sock_path)

    env = dict(os.environ)
    env[ENV_SOCK_PATH] = sock_path
    if session_id is not None:
        env[ENV_SESSION_ID] = str(session_id)

    new_argv = [os.path.basename(real)] + argv[1:]
    mode = _select_mode()
    if mode == "pty" and session_id is not None:
        return _run_pty(real, new_argv, env, sock_path, plugin_name, session_id)
    return _run_exec(real, new_argv, env)


def _run_exec(real: str, new_argv: List[str], env: dict) -> int:
    try:
        os.execvpe(real, new_argv, env)
    except OSError as exc:
        print(f"vibe-bridge wrapper: failed to exec {real}: {exc}", file=sys.stderr)
        return 126
    return 0  # unreachable


def _run_pty(
    real: str,
    new_argv: List[str],
    env: dict,
    sock_path: str,
    plugin_name: str,
    session_id: int,
) -> int:
    """Run real CLI under a PTY; tee output to the daemon as CMD_VT100_STREAM."""
    # Imports are local so exec mode stays free of the PTY dependency chain.
    from .pty_runner import run_with_pty

    # The wrapper-as-parent owns its own PluginClient. We DO NOT call
    # ``acquire_session`` again — we adopt the sid we already have so all
    # forwarded VT100 lands in the right window.
    plugin = PluginClient(plugin_name=plugin_name, sock_path=sock_path)
    plugin.adopt_session(session_id)
    try:
        plugin.connect()
    except OSError:
        # Daemon went away between handshake and adoption; fall back to exec.
        return _run_exec(real, new_argv, env)

    forwarder = Forwarder(plugin.send_vt100)
    forwarder.start()

    def on_output(chunk: bytes) -> None:
        forwarder.push(chunk)

    real_argv = [real] + new_argv[1:]  # argv[0] should be the program path
    try:
        return run_with_pty(real_argv, env=env, on_output=on_output)
    finally:
        forwarder.stop(timeout=0.5)
        plugin.close()


def _acquire_session(plugin_name: str, sock_path: str) -> Optional[int]:
    if not ensure_daemon_running(sock_path, timeout=3.0):
        print(
            "vibe-bridge wrapper: daemon unreachable, running without session",
            file=sys.stderr,
        )
        return None

    existing = _existing_session_from_env()
    if existing is not None:
        return existing

    try:
        with PluginClient(plugin_name=plugin_name, sock_path=sock_path) as p:
            sid = p.acquire_session(timeout=2.0)
            return sid
    except (PluginError, OSError) as exc:
        print(
            f"vibe-bridge wrapper: session handshake failed ({exc}); "
            "running without session",
            file=sys.stderr,
        )
        return None


def _exec_real(plugin_name: str, argv: List[str], self_paths: List[str]) -> int:
    real = find_real_binary(plugin_name, exclude_paths=self_paths)
    if real is None:
        print(f"vibe-bridge wrapper: no real `{plugin_name}` on PATH", file=sys.stderr)
        return 127
    new_argv = [os.path.basename(real)] + argv[1:]
    try:
        os.execvp(real, new_argv)
    except OSError as exc:
        print(f"vibe-bridge wrapper: failed to exec {real}: {exc}", file=sys.stderr)
        return 126
    return 0  # unreachable
