"""Mock HID transport over a Unix domain socket.

The daemon side (``MockHidServer``) accepts plugin connections, dispatches each
incoming packet through a handler callback, and lets the handler push packets
back to the originating client. The plugin side (``MockHidClient``) speaks the
same packet format carried by real HID reports, so mock and hidraw modes share
one protocol.
"""

from __future__ import annotations

import os
import socket
import struct
import threading
from typing import Callable, List, Optional

from .hid_protocol import HEADER_SIZE, Packet, ProtocolError
from .transport import (
    LENGTH_PREFIX,
    LENGTH_PREFIX_SIZE,
    Transport,
    TransportClosed,
)

DEFAULT_SOCK_PATH = "/tmp/vibe-bridge.sock"


def _recv_exact(sock: socket.socket, n: int) -> bytes:
    buf = bytearray()
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise TransportClosed("peer closed mid-frame")
        buf.extend(chunk)
    return bytes(buf)


def _read_packet(sock: socket.socket) -> Packet:
    header = _recv_exact(sock, LENGTH_PREFIX_SIZE)
    (declared,) = struct.unpack(LENGTH_PREFIX, header)
    body = _recv_exact(sock, declared)
    if len(body) < HEADER_SIZE:
        raise ProtocolError(f"frame smaller than HID header: {declared}")
    return Packet.decode(body)


def _write_packet(sock: socket.socket, packet: Packet) -> None:
    raw = packet.encode()
    sock.sendall(struct.pack(LENGTH_PREFIX, len(raw)) + raw)


# ---------------------------------------------------------------- client API


class MockHidClient(Transport):
    """Plugin-side transport. Connects to MockHidServer."""

    def __init__(self, sock_path: str = DEFAULT_SOCK_PATH) -> None:
        self._sock_path = sock_path
        self._sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._sock.connect(sock_path)
        self._lock = threading.Lock()

    def send_packet(self, packet: Packet) -> None:
        with self._lock:
            _write_packet(self._sock, packet)

    def recv_packet(self, timeout: Optional[float] = None) -> Optional[Packet]:
        self._sock.settimeout(timeout)
        try:
            return _read_packet(self._sock)
        except socket.timeout:
            return None
        except TransportClosed:
            return None
        finally:
            self._sock.settimeout(None)

    def close(self) -> None:
        try:
            self._sock.close()
        except OSError:
            pass


# ---------------------------------------------------------------- server API


class ClientHandle:
    """Server-side handle for one connected plugin.

    Use ``send`` to push a packet back to the originating plugin.
    """

    def __init__(self, sock: socket.socket, addr: object) -> None:
        self._sock = sock
        self._addr = addr
        self._lock = threading.Lock()
        self.client_id = id(self)

    def send(self, packet: Packet) -> None:
        with self._lock:
            _write_packet(self._sock, packet)

    def close(self) -> None:
        try:
            self._sock.close()
        except OSError:
            pass


PacketHandler = Callable[[Packet, ClientHandle], None]


class MockHidServer:
    """Listens on a Unix socket, dispatches each packet to ``handler``.

    Spawns one thread per client. The handler runs on that thread; it must be
    thread-safe with respect to other clients but does not need to serialize
    with itself, as the socket is read sequentially per client.
    """

    def __init__(
        self,
        handler: PacketHandler,
        *,
        sock_path: str = DEFAULT_SOCK_PATH,
        backlog: int = 16,
    ) -> None:
        self._handler = handler
        self._sock_path = sock_path
        self._backlog = backlog
        self._sock: Optional[socket.socket] = None
        self._accept_thread: Optional[threading.Thread] = None
        self._client_threads: List[threading.Thread] = []
        self._clients: List[ClientHandle] = []
        self._stop = threading.Event()
        self._lock = threading.Lock()

    @property
    def sock_path(self) -> str:
        return self._sock_path

    def start(self) -> None:
        if self._sock is not None:
            raise RuntimeError("server already started")
        try:
            os.unlink(self._sock_path)
        except FileNotFoundError:
            pass

        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.bind(self._sock_path)
        sock.listen(self._backlog)
        sock.settimeout(0.5)
        self._sock = sock

        self._accept_thread = threading.Thread(
            target=self._accept_loop, name="vibe-bridge-accept", daemon=True
        )
        self._accept_thread.start()

    def stop(self) -> None:
        self._stop.set()
        if self._sock is not None:
            try:
                self._sock.close()
            except OSError:
                pass
            self._sock = None
        with self._lock:
            for c in self._clients:
                c.close()
            self._clients.clear()
        if self._accept_thread is not None:
            self._accept_thread.join(timeout=1.0)
        for t in self._client_threads:
            t.join(timeout=1.0)
        try:
            os.unlink(self._sock_path)
        except FileNotFoundError:
            pass

    def broadcast(self, packet: Packet) -> None:
        with self._lock:
            clients = list(self._clients)
        for c in clients:
            try:
                c.send(packet)
            except (OSError, TransportClosed):
                pass

    # ------------------------------------------------------------ internals

    def _accept_loop(self) -> None:
        while not self._stop.is_set() and self._sock is not None:
            try:
                conn, addr = self._sock.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            handle = ClientHandle(conn, addr)
            with self._lock:
                self._clients.append(handle)
            t = threading.Thread(
                target=self._client_loop,
                args=(handle,),
                name=f"vibe-bridge-client-{handle.client_id}",
                daemon=True,
            )
            self._client_threads.append(t)
            t.start()

    def _client_loop(self, handle: ClientHandle) -> None:
        sock = handle._sock
        try:
            while not self._stop.is_set():
                try:
                    packet = _read_packet(sock)
                except TransportClosed:
                    break
                except (OSError, ProtocolError):
                    break
                try:
                    self._handler(packet, handle)
                except Exception:
                    # Handler errors must not kill the read loop. Tests/log
                    # facilities should surface them via the handler itself.
                    continue
        finally:
            with self._lock:
                if handle in self._clients:
                    self._clients.remove(handle)
            handle.close()
