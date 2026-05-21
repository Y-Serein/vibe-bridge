"""Mock/local packet transport over Unix sockets or loopback TCP.

The daemon side (``MockHidServer``) accepts plugin connections, dispatches each
incoming packet through a handler callback, and lets the handler push packets
back to the originating client. The plugin side (``MockHidClient``) speaks the
same packet format carried by real HID reports, so mock and hidraw modes share
one protocol.

Unix sockets are the default development path.  ``tcp://HOST:PORT`` endpoints
use the same length-prefixed packet framing and are the Windows product IPC
path, because native Windows cannot rely on the Linux ``/tmp/*.sock`` contract.
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
DEFAULT_TCP_ENDPOINT = "tcp://127.0.0.1:8765"


def is_tcp_endpoint(endpoint: str) -> bool:
    return endpoint.startswith("tcp://")


def parse_tcp_endpoint(endpoint: str) -> tuple[str, int]:
    if not is_tcp_endpoint(endpoint):
        raise ValueError(f"not a tcp endpoint: {endpoint}")
    host_port = endpoint[len("tcp://") :]
    host, sep, port_s = host_port.rpartition(":")
    if not sep or not host:
        raise ValueError(f"tcp endpoint must be tcp://HOST:PORT: {endpoint}")
    port = int(port_s)
    if not (1 <= port <= 65535):
        raise ValueError(f"tcp port out of range: {endpoint}")
    return host, port


def connect_packet_socket(endpoint: str, *, timeout: Optional[float] = None) -> socket.socket:
    if is_tcp_endpoint(endpoint):
        host, port = parse_tcp_endpoint(endpoint)
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        if timeout is not None:
            sock.settimeout(timeout)
        sock.connect((host, port))
        sock.settimeout(None)
        return sock
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    if timeout is not None:
        sock.settimeout(timeout)
    sock.connect(endpoint)
    sock.settimeout(None)
    return sock


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
        self._sock = connect_packet_socket(sock_path)
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
DisconnectHandler = Callable[[ClientHandle], None]


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
        on_disconnect: Optional[DisconnectHandler] = None,
    ) -> None:
        self._handler = handler
        self._sock_path = sock_path
        self._backlog = backlog
        self._on_disconnect = on_disconnect
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
        if is_tcp_endpoint(self._sock_path):
            host, port = parse_tcp_endpoint(self._sock_path)
            sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            sock.bind((host, port))
        else:
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
        if not is_tcp_endpoint(self._sock_path):
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
            if not self._stop.is_set() and self._on_disconnect is not None:
                try:
                    self._on_disconnect(handle)
                except Exception:
                    pass
            handle.close()
