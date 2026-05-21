"""Plugin SDK: small wrapper that handles session handshake and invalidation.

Plugins call::

    with PluginClient(plugin_name="codex") as p:
        p.acquire_session()
        p.send_vt100("hello\\r\\n")

The client runs a background reader thread; when a ``CMD_SESSION_INVALID``
arrives it either auto-reacquires (default) or surfaces the event through the
``on_invalidate`` callback.
"""

from __future__ import annotations

import os
import threading
import time
from typing import Callable, Optional

from .hid_protocol import (
    Cmd,
    Packet,
    Status,
    make_request_session,
    stream_iter_packets,
)
from .mock_hid import DEFAULT_SOCK_PATH, MockHidClient
from .transport import TransportClosed

InvalidateCallback = Callable[[int, Status], None]
PacketCallback = Callable[[Packet], None]
ENV_SOCK_PATH = "VIBE_SOCK_PATH"


class PluginError(Exception):
    pass


class PluginClient:
    def __init__(
        self,
        *,
        plugin_name: str,
        sock_path: Optional[str] = None,
        auto_reacquire: bool = True,
        on_invalidate: Optional[InvalidateCallback] = None,
        on_board_packet: Optional[PacketCallback] = None,
    ) -> None:
        self._plugin_name = plugin_name
        self._sock_path = sock_path or os.environ.get(ENV_SOCK_PATH, DEFAULT_SOCK_PATH)
        self._auto_reacquire = auto_reacquire
        self._on_invalidate = on_invalidate
        self._on_board_packet = on_board_packet

        self._client: Optional[MockHidClient] = None
        self._reader: Optional[threading.Thread] = None
        self._stop = threading.Event()

        self._sid: Optional[int] = None
        self._sid_lock = threading.Lock()
        self._sid_cond = threading.Condition(self._sid_lock)

    # ------------------------------------------------------------- context

    def __enter__(self) -> "PluginClient":
        self.connect()
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.close()

    # ----------------------------------------------------------- connection

    def connect(self) -> None:
        if self._client is not None:
            return
        self._client = MockHidClient(self._sock_path)
        self._stop.clear()
        self._reader = threading.Thread(
            target=self._reader_loop, name=f"vibe-plugin-{self._plugin_name}", daemon=True
        )
        self._reader.start()

    def close(self) -> None:
        self._stop.set()
        if self._client is not None:
            self._client.close()
            self._client = None
        if self._reader is not None:
            self._reader.join(timeout=0.5)
            self._reader = None

    # -------------------------------------------------------------- session

    @property
    def session_id(self) -> Optional[int]:
        with self._sid_lock:
            return self._sid

    def acquire_session(self, timeout: float = 2.0) -> int:
        """Request a session id from the daemon; blocks until allocated."""
        if self._client is None:
            raise PluginError("client not connected")
        self._client.send_packet(make_request_session(hint=self._plugin_name.encode("utf-8")))
        deadline = time.time() + timeout
        with self._sid_cond:
            while self._sid is None:
                remaining = deadline - time.time()
                if remaining <= 0:
                    raise PluginError(f"session handshake timed out ({timeout}s)")
                self._sid_cond.wait(timeout=remaining)
            return self._sid

    def adopt_session(self, sid: int) -> None:
        """Use an already-allocated sid without re-running the handshake.

        Call this *before* ``connect`` (or any send/recv) when a parent process
        passed the sid through ``$VIBE_SESSION_ID`` and the bridge daemon
        already owns it. ``send_vt100`` calls will tag each packet with this
        sid and the daemon's owner-rebind logic will route invalidation
        notifications back to this client.
        """
        with self._sid_cond:
            self._sid = sid
            self._sid_cond.notify_all()

    def send_vt100(self, data) -> None:
        if isinstance(data, str):
            data = data.encode("utf-8")
        sid = self.session_id
        if sid is None:
            raise PluginError("no session id; call acquire_session() first")
        if self._client is None:
            raise PluginError("client not connected")
        for pkt in stream_iter_packets(sid, data):
            self._client.send_packet(pkt)

    def send_packet(self, packet: Packet) -> None:
        if self._client is None:
            raise PluginError("client not connected")
        self._client.send_packet(packet)

    def set_board_packet_handler(self, callback: Optional[PacketCallback]) -> None:
        self._on_board_packet = callback

    # ------------------------------------------------------------- internals

    def _reader_loop(self) -> None:
        client = self._client
        if client is None:
            return
        while not self._stop.is_set():
            try:
                pkt = client.recv_packet(timeout=0.5)
            except TransportClosed:
                break
            except OSError:
                break
            if pkt is None:
                continue
            self._dispatch(pkt)

    def _dispatch(self, pkt: Packet) -> None:
        if pkt.command == int(Cmd.SESSION_RESPONSE):
            with self._sid_cond:
                self._sid = pkt.session_id
                self._sid_cond.notify_all()
            return
        if pkt.command == int(Cmd.SESSION_INVALID):
            status = Status(pkt.payload[0]) if pkt.payload else Status.INVALID
            with self._sid_lock:
                current_sid = self._sid
            if pkt.session_id not in (0, current_sid):
                return
            with self._sid_cond:
                self._sid = None
                self._sid_cond.notify_all()
            if self._on_invalidate is not None:
                try:
                    self._on_invalidate(pkt.session_id, status)
                except Exception:
                    pass
            if self._auto_reacquire and self._client is not None:
                try:
                    self._client.send_packet(
                        make_request_session(hint=self._plugin_name.encode("utf-8"))
                    )
                except OSError:
                    pass
            return
        if pkt.command in (int(Cmd.KEY_EVENT), int(Cmd.ENCODER_EVENT)):
            if self._on_board_packet is not None:
                try:
                    self._on_board_packet(pkt)
                except Exception:
                    pass
            return
