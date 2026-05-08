"""Per-session VT100 buffers and a single 'active' window selector.

Each session owns an append-only byte buffer of VT100 output. The router also
tracks which session is currently active; only the active session's stream is
mirrored to the screen sink (so multiple plugins do not cross-contaminate the
LCD output). All public methods are thread-safe.

When the active window switches, the router clears the screen sink and replays
the new session's accumulated buffer so the LCD shows that session's last
state instead of the previous window's frozen frame.
"""

from __future__ import annotations

import threading
from collections import deque
from typing import Callable, Deque, Dict, List, Optional, Tuple

ScreenSink = Callable[[int, bytes], None]
"""Called with (session_id, vt100_bytes) every time the active session writes."""

DEFAULT_BUFFER_BYTES = 64 * 1024
SCREEN_CLEAR = b"\x1b[2J\x1b[H"


class Vt100Router:
    def __init__(
        self,
        *,
        buffer_bytes: int = DEFAULT_BUFFER_BYTES,
        screen_sink: Optional[ScreenSink] = None,
    ) -> None:
        self._buffer_bytes = buffer_bytes
        self._screen_sink = screen_sink
        self._buffers: Dict[int, Deque[bytes]] = {}
        self._sizes: Dict[int, int] = {}
        self._active: Optional[int] = None
        self._lock = threading.Lock()

    # ---------------------------------------------------------------- config

    def set_screen_sink(self, sink: Optional[ScreenSink]) -> None:
        with self._lock:
            self._screen_sink = sink

    # ------------------------------------------------------------ membership

    def register(self, sid: int) -> None:
        with self._lock:
            self._buffers.setdefault(sid, deque())
            self._sizes.setdefault(sid, 0)
            if self._active is None:
                self._active = sid

    def unregister(self, sid: int) -> None:
        replacement: Optional[int] = None
        replay: Optional[bytes] = None
        sink: Optional[ScreenSink] = None
        with self._lock:
            self._buffers.pop(sid, None)
            self._sizes.pop(sid, None)
            if self._active == sid:
                replacement = next(iter(self._buffers), None)
                self._active = replacement
                sink = self._screen_sink
                if sink is not None:
                    if replacement is None:
                        replay = SCREEN_CLEAR
                    else:
                        buf = self._buffers.get(replacement)
                        body = b"".join(buf) if buf else b""
                        replay = SCREEN_CLEAR + body
        if sink is not None and replay is not None:
            try:
                sink(replacement if replacement is not None else 0, replay)
            except Exception:
                pass

    # ------------------------------------------------------------ activation

    def set_active(self, sid: Optional[int]) -> bool:
        """Switch the active window. On a real switch (different sid), flush
        a clear sequence followed by the new sid's full buffer to the sink so
        the screen shows that session's last state.
        """
        with self._lock:
            if sid is None:
                if self._active is None:
                    return True
                self._active = None
                sink = self._screen_sink
                replay: Optional[bytes] = SCREEN_CLEAR if sink is not None else None
                target_sid: Optional[int] = None
            else:
                if sid not in self._buffers:
                    return False
                if self._active == sid:
                    return True
                self._active = sid
                sink = self._screen_sink
                if sink is None:
                    replay = None
                    target_sid = sid
                else:
                    buf = self._buffers.get(sid)
                    body = b"".join(buf) if buf else b""
                    replay = SCREEN_CLEAR + body
                    target_sid = sid

        if sink is not None and replay is not None:
            try:
                sink(target_sid if target_sid is not None else 0, replay)
            except Exception:
                pass
        return True

    def active(self) -> Optional[int]:
        with self._lock:
            return self._active

    # -------------------------------------------------------------- streaming

    def append(self, sid: int, data: bytes) -> None:
        if not data:
            return
        with self._lock:
            self._buffers.setdefault(sid, deque())
            self._sizes.setdefault(sid, 0)
            self._buffers[sid].append(data)
            self._sizes[sid] += len(data)
            self._evict_locked(sid)
            sink = self._screen_sink
            is_active = self._active == sid
        if sink is not None and is_active:
            sink(sid, data)

    def snapshot(self, sid: int) -> bytes:
        with self._lock:
            buf = self._buffers.get(sid)
            if not buf:
                return b""
            return b"".join(buf)

    def buffer_sizes(self) -> List[Tuple[int, int]]:
        with self._lock:
            return sorted(self._sizes.items())

    # -------------------------------------------------------------- internals

    def _evict_locked(self, sid: int) -> None:
        buf = self._buffers[sid]
        while self._sizes[sid] > self._buffer_bytes and buf:
            dropped = buf.popleft()
            self._sizes[sid] -= len(dropped)
