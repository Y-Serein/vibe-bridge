"""Run a child process under a PTY and tee its output.

Why this exists
---------------
The shell wrapper used to ``execvp`` straight into the real CLI, which gave us
nothing to forward to the daemon. ``run_with_pty`` keeps the wrapper alive as a
parent that owns the PTY master end:

- bytes from PTY master  → user's stdout  + ``on_output(bytes)`` callback
- bytes from user's stdin → PTY master    (so keyboard, Ctrl-C, escapes work)

``on_output`` is the hook that the wrapper attaches to a ``Forwarder``, which
publishes ``CMD_VT100_STREAM`` packets to the daemon.

Important behaviours
--------------------
- The PTY is sized to match the parent's stdin window (``TIOCGWINSZ`` →
  ``TIOCSWINSZ`` on the master). A SIGWINCH handler keeps it in sync if the
  user resizes their terminal.
- The parent's stdin is put in raw mode so escape sequences and Ctrl-C bytes
  reach the child unmodified — the child's PTY line discipline turns them into
  signals to the foreground process group, which is the child.
- All terminal modifications are reverted in a ``finally`` block: even on a
  panic, the user's shell is left in cooked mode.
"""

from __future__ import annotations

import errno
import fcntl
import os
import select
import signal
import struct
import sys
import termios
import tty
from typing import Callable, List, Optional, Sequence

ChunkCallback = Callable[[bytes], None]

_READ_CHUNK = 4096


def _isatty(fd: int) -> bool:
    try:
        return os.isatty(fd)
    except OSError:
        return False


def _get_winsize(fd: int) -> Optional[bytes]:
    try:
        return fcntl.ioctl(fd, termios.TIOCGWINSZ, b"\x00" * 8)
    except OSError:
        return None


def _set_winsize(fd: int, packed: bytes) -> None:
    try:
        fcntl.ioctl(fd, termios.TIOCSWINSZ, packed)
    except OSError:
        pass


def run_with_pty(
    argv: Sequence[str],
    *,
    env: Optional[dict] = None,
    on_output: Optional[ChunkCallback] = None,
    stdin_fd: Optional[int] = None,
    stdout_fd: Optional[int] = None,
) -> int:
    """Spawn ``argv`` under a PTY; tee output to stdout + ``on_output``.

    Returns the child's exit code (or ``128 + signum`` if killed by signal).
    Raises ``OSError`` on fork failures.
    """
    argv = list(argv)
    if not argv:
        raise ValueError("argv must not be empty")
    in_fd = stdin_fd if stdin_fd is not None else sys.stdin.fileno()
    out_fd = stdout_fd if stdout_fd is not None else sys.stdout.fileno()

    # ``pty.fork`` allocates a PTY pair, forks, attaches the slave to the
    # child's stdio + makes it the controlling tty, and returns the master end
    # to the parent.
    import pty  # imported here to keep top-level imports portable

    pid, master_fd = pty.fork()
    if pid == 0:
        # Child: exec the target. Flush nothing; stdio is the slave PTY now.
        try:
            if env is not None:
                os.execvpe(argv[0], argv, env)
            else:
                os.execvp(argv[0], argv)
        except OSError as exc:
            sys.stderr.write(f"vibe-bridge pty: exec failed: {exc}\n")
            os._exit(126)
        os._exit(127)  # unreachable

    # Parent. Put parent stdin in raw mode (if it's a tty) and copy the parent
    # window size onto the PTY master.
    parent_in_is_tty = _isatty(in_fd)
    saved_termios: Optional[list] = None

    initial_winsize = _get_winsize(in_fd) if parent_in_is_tty else None
    if initial_winsize is not None:
        _set_winsize(master_fd, initial_winsize)

    def _on_winch(signum, frame):  # noqa: ARG001
        ws = _get_winsize(in_fd) if parent_in_is_tty else None
        if ws is not None:
            _set_winsize(master_fd, ws)
        # Forward SIGWINCH to the child too so curses-style apps re-render.
        try:
            os.kill(pid, signal.SIGWINCH)
        except ProcessLookupError:
            pass

    prev_winch_handler = signal.signal(signal.SIGWINCH, _on_winch) if parent_in_is_tty else None

    try:
        if parent_in_is_tty:
            saved_termios = termios.tcgetattr(in_fd)
            tty.setraw(in_fd)

        _io_loop(master_fd, in_fd, out_fd, on_output)
    finally:
        if saved_termios is not None:
            try:
                termios.tcsetattr(in_fd, termios.TCSAFLUSH, saved_termios)
            except OSError:
                pass
        if prev_winch_handler is not None:
            signal.signal(signal.SIGWINCH, prev_winch_handler)
        try:
            os.close(master_fd)
        except OSError:
            pass

    # Reap child.
    try:
        _, status = os.waitpid(pid, 0)
    except ChildProcessError:
        return 0
    if os.WIFEXITED(status):
        return os.WEXITSTATUS(status)
    if os.WIFSIGNALED(status):
        return 128 + os.WTERMSIG(status)
    return 1


def _io_loop(
    master_fd: int,
    in_fd: int,
    out_fd: int,
    on_output: Optional[ChunkCallback],
) -> None:
    rlist: List[int] = [master_fd]
    if in_fd >= 0:
        rlist.append(in_fd)
    while True:
        try:
            r, _, _ = select.select(rlist, [], [], 1.0)
        except InterruptedError:
            continue
        except OSError as exc:
            if exc.errno == errno.EBADF:
                return
            raise

        if master_fd in r:
            try:
                data = os.read(master_fd, _READ_CHUNK)
            except OSError as exc:
                # Linux returns EIO when the slave side has closed.
                if exc.errno == errno.EIO:
                    return
                raise
            if not data:
                return
            try:
                _write_all(out_fd, data)
            except OSError:
                pass
            if on_output is not None:
                try:
                    on_output(data)
                except Exception:
                    # Forwarder errors must not break the user's terminal.
                    pass

        if in_fd in r:
            try:
                data = os.read(in_fd, _READ_CHUNK)
            except OSError:
                rlist = [master_fd]
                continue
            if not data:
                # parent stdin closed; let child finish on its own
                rlist = [master_fd]
                continue
            try:
                _write_all(master_fd, data)
            except OSError:
                return


def _write_all(fd: int, data: bytes) -> None:
    view = memoryview(data)
    while view:
        n = os.write(fd, view)
        if n <= 0:
            raise OSError("short write to fd")
        view = view[n:]
