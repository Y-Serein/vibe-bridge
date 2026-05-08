"""Session manager: allocates uint16 session IDs and tracks per-session state.

Allocation rules (per request.md §3):
- ``session_id == 0`` is reserved as a broadcast / unassigned marker.
- Default cap is 256 concurrent sessions (sids 1..256).
- When the pool is full, the least-recently-active session is reclaimed to make
  room and its previous owner is notified via the invalidation callback with
  status ``Status.RECLAIMED``.
- Sessions idle longer than ``ttl_seconds`` are reaped on demand and notified
  with status ``Status.EXPIRED``.

Thread-safe: all public methods take a single internal lock.
"""

from __future__ import annotations

import threading
import time
from dataclasses import dataclass, field
from typing import Callable, Dict, List, Optional, Tuple

from .hid_protocol import Status

DEFAULT_MAX_SESSIONS = 256
DEFAULT_TTL_SECONDS = 30 * 24 * 60 * 60  # 30 days

InvalidationCallback = Callable[[int, Status], None]


@dataclass
class Session:
    sid: int
    plugin: str
    cwd: str
    created_ms: int
    last_active_ms: int
    context: dict = field(default_factory=dict)

    def touch(self, now_ms: Optional[int] = None) -> None:
        self.last_active_ms = now_ms if now_ms is not None else _now_ms()


def _now_ms() -> int:
    return int(time.time() * 1000)


class SessionManager:
    """Allocates and tracks sessions. See module docstring."""

    def __init__(
        self,
        *,
        max_sessions: int = DEFAULT_MAX_SESSIONS,
        ttl_seconds: int = DEFAULT_TTL_SECONDS,
        clock_ms: Callable[[], int] = _now_ms,
    ) -> None:
        if max_sessions <= 0 or max_sessions > 0xFFFF:
            raise ValueError(f"max_sessions out of range: {max_sessions}")
        self._max = max_sessions
        self._ttl_ms = ttl_seconds * 1000
        self._clock = clock_ms
        self._lock = threading.RLock()
        self._sessions: Dict[int, Session] = {}
        self._next_sid = 1
        self._on_invalidate: Optional[InvalidationCallback] = None

    # ------------------------------------------------------------------ config

    def set_invalidation_callback(self, cb: Optional[InvalidationCallback]) -> None:
        with self._lock:
            self._on_invalidate = cb

    # ---------------------------------------------------------------- queries

    def get(self, sid: int) -> Optional[Session]:
        with self._lock:
            return self._sessions.get(sid)

    def all_sessions(self) -> List[Session]:
        with self._lock:
            return sorted(self._sessions.values(), key=lambda s: s.sid)

    def __len__(self) -> int:
        with self._lock:
            return len(self._sessions)

    # ------------------------------------------------------------- mutations

    def request_session(
        self, plugin: str, cwd: str = ""
    ) -> Tuple[int, Status, List[int]]:
        """Allocate a fresh session id.

        Returns ``(sid, status, evicted_sids)`` where ``evicted_sids`` is a list
        of sessions reclaimed to make room (already notified via callback).
        """
        with self._lock:
            self._reap_expired_locked()
            evicted: List[int] = []
            if len(self._sessions) >= self._max:
                victim = self._pick_lru_locked()
                if victim is not None:
                    self._invalidate_locked(victim, Status.RECLAIMED)
                    evicted.append(victim)
                else:
                    return (0, Status.POOL_FULL, evicted)

            sid = self._allocate_sid_locked()
            now = self._clock()
            self._sessions[sid] = Session(
                sid=sid,
                plugin=plugin,
                cwd=cwd,
                created_ms=now,
                last_active_ms=now,
            )
            return (sid, Status.CREATED, evicted)

    def adopt_session(
        self, sid: int, plugin: str, cwd: str = ""
    ) -> Tuple[int, Status, List[int]]:
        """Register a session id allocated by the board firmware.

        Real-HID mode treats the board as the authority for session ids. The
        host still mirrors the session table so VT100 routing, owner rebinding,
        and state dumps work exactly like mock mode.
        """
        if sid <= 0 or sid > self._max:
            return (0, Status.INVALID, [])

        with self._lock:
            self._reap_expired_locked()
            evicted: List[int] = []

            if sid in self._sessions:
                self._invalidate_locked(sid, Status.RECLAIMED)
                evicted.append(sid)
            elif len(self._sessions) >= self._max:
                victim = self._pick_lru_locked()
                if victim is not None:
                    self._invalidate_locked(victim, Status.RECLAIMED)
                    evicted.append(victim)
                else:
                    return (0, Status.POOL_FULL, evicted)

            now = self._clock()
            self._sessions[sid] = Session(
                sid=sid,
                plugin=plugin,
                cwd=cwd,
                created_ms=now,
                last_active_ms=now,
            )
            self._advance_next_sid_locked()
            return (sid, Status.CREATED, evicted)

    def touch(self, sid: int) -> bool:
        """Update last-active timestamp. Returns False if sid is unknown."""
        with self._lock:
            sess = self._sessions.get(sid)
            if sess is None:
                return False
            sess.touch(self._clock())
            return True

    def invalidate(self, sid: int, status: Status = Status.INVALID) -> bool:
        with self._lock:
            if sid not in self._sessions:
                return False
            self._invalidate_locked(sid, status)
            return True

    def reap_expired(self) -> List[int]:
        """Remove sessions idle longer than the TTL; returns reaped sids."""
        with self._lock:
            return self._reap_expired_locked()

    # ------------------------------------------------------------- internals

    def _reap_expired_locked(self) -> List[int]:
        now = self._clock()
        cutoff = now - self._ttl_ms
        expired = [s.sid for s in self._sessions.values() if s.last_active_ms < cutoff]
        for sid in expired:
            self._invalidate_locked(sid, Status.EXPIRED)
        return expired

    def _pick_lru_locked(self) -> Optional[int]:
        if not self._sessions:
            return None
        return min(self._sessions.values(), key=lambda s: s.last_active_ms).sid

    def _invalidate_locked(self, sid: int, status: Status) -> None:
        self._sessions.pop(sid, None)
        cb = self._on_invalidate
        if cb is not None:
            try:
                cb(sid, status)
            except Exception:
                # Callback failures must not corrupt the session table.
                pass

    def _allocate_sid_locked(self) -> int:
        # Linear probe from _next_sid; cap is small enough that this is fine.
        for _ in range(self._max):
            candidate = self._next_sid
            self._advance_next_sid_locked()
            if candidate not in self._sessions:
                return candidate
        # Should be unreachable: we already evicted to make room.
        raise RuntimeError("session pool exhausted after eviction")

    def _advance_next_sid_locked(self) -> None:
        self._next_sid = self._next_sid + 1
        if self._next_sid > self._max:
            self._next_sid = 1
