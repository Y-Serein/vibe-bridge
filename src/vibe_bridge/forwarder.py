"""Async byte forwarder: PTY master output -> daemon CMD_VT100_STREAM.

The PTY io loop must never block on the daemon: a slow or dead daemon would
freeze the user's terminal. ``Forwarder`` owns a bounded queue and a worker
thread; the io loop calls ``push(data)`` (non-blocking, drops oldest on
overflow) and the worker drains to ``PluginClient.send_vt100``.

Failures during ``send_vt100`` are swallowed — the user's terminal is more
important than the bridge. We log the count via ``stats()`` so callers (and
tests) can detect backpressure.
"""

from __future__ import annotations

import queue
import threading
from dataclasses import dataclass
from typing import Callable, Optional

# Sentinel used to wake the worker for shutdown.
_STOP = object()


@dataclass
class ForwarderStats:
    pushed: int = 0
    sent: int = 0
    dropped: int = 0
    errors: int = 0


SendCallback = Callable[[bytes], None]


class Forwarder:
    """Drain queued chunks of bytes to a send callback on a worker thread."""

    def __init__(
        self,
        send: SendCallback,
        *,
        max_queue: int = 256,
        on_error: Optional[Callable[[BaseException], None]] = None,
    ) -> None:
        self._send = send
        self._queue: "queue.Queue[object]" = queue.Queue(maxsize=max_queue)
        self._stop = threading.Event()
        self._thread: Optional[threading.Thread] = None
        self._stats = ForwarderStats()
        self._stats_lock = threading.Lock()
        self._on_error = on_error

    # ------------------------------------------------------------- lifecycle

    def start(self) -> None:
        if self._thread is not None:
            return
        t = threading.Thread(target=self._loop, name="vibe-bridge-forwarder", daemon=True)
        self._thread = t
        t.start()

    def stop(self, *, timeout: float = 1.0) -> None:
        if self._thread is None:
            return
        self._stop.set()
        try:
            self._queue.put_nowait(_STOP)
        except queue.Full:
            # Drop one and retry; the worker will pick up _STOP after.
            try:
                self._queue.get_nowait()
            except queue.Empty:
                pass
            try:
                self._queue.put_nowait(_STOP)
            except queue.Full:
                pass
        self._thread.join(timeout=timeout)
        self._thread = None

    # ----------------------------------------------------------- io surface

    def push(self, data: bytes) -> None:
        if not data:
            return
        try:
            self._queue.put_nowait(data)
        except queue.Full:
            # Drop the oldest queued chunk to make room.
            try:
                self._queue.get_nowait()
                with self._stats_lock:
                    self._stats.dropped += 1
            except queue.Empty:
                pass
            try:
                self._queue.put_nowait(data)
            except queue.Full:
                with self._stats_lock:
                    self._stats.dropped += 1
                return
        with self._stats_lock:
            self._stats.pushed += 1

    def stats(self) -> ForwarderStats:
        with self._stats_lock:
            return ForwarderStats(
                pushed=self._stats.pushed,
                sent=self._stats.sent,
                dropped=self._stats.dropped,
                errors=self._stats.errors,
            )

    # -------------------------------------------------------------- internals

    def _loop(self) -> None:
        while True:
            item = self._queue.get()
            if item is _STOP:
                return
            try:
                self._send(item)  # type: ignore[arg-type]
                with self._stats_lock:
                    self._stats.sent += 1
            except BaseException as exc:  # noqa: BLE001 — forwarder must never die
                with self._stats_lock:
                    self._stats.errors += 1
                if self._on_error is not None:
                    try:
                        self._on_error(exc)
                    except Exception:
                        pass
            if self._stop.is_set() and self._queue.empty():
                return
