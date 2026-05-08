"""Transport abstractions.

The wire payload on every transport is exactly one ``hid_protocol.Packet``. The
mock transport wraps each packet in a 4-byte little-endian length prefix so it
can ride a SOCK_STREAM Unix socket; ``HidrawTransport`` writes the same packet
bytes to ``/dev/hidraw*`` directly (real HID gives us per-report
framing for free, so no length prefix is needed there).

Keeping this thin abstraction means the daemon, plugin client, and tests all
target the same ``Transport.send_packet`` / ``Transport.recv_packet`` surface.
"""

from __future__ import annotations

import struct
from abc import ABC, abstractmethod
from typing import Optional

from .hid_protocol import HEADER_SIZE, Packet, ProtocolError

LENGTH_PREFIX = "<I"
LENGTH_PREFIX_SIZE = struct.calcsize(LENGTH_PREFIX)


class TransportClosed(Exception):
    """Raised when the peer hangs up mid-frame."""


class Transport(ABC):
    @abstractmethod
    def send_packet(self, packet: Packet) -> None: ...

    @abstractmethod
    def recv_packet(self, timeout: Optional[float] = None) -> Optional[Packet]: ...

    @abstractmethod
    def close(self) -> None: ...


def encode_framed(packet: Packet) -> bytes:
    raw = packet.encode()
    return struct.pack(LENGTH_PREFIX, len(raw)) + raw


def decode_framed(buf: bytes) -> Packet:
    if len(buf) < LENGTH_PREFIX_SIZE + HEADER_SIZE:
        raise ProtocolError(f"framed buffer too short: {len(buf)}")
    (declared,) = struct.unpack(LENGTH_PREFIX, buf[:LENGTH_PREFIX_SIZE])
    body = buf[LENGTH_PREFIX_SIZE : LENGTH_PREFIX_SIZE + declared]
    if len(body) != declared:
        raise ProtocolError(
            f"framed body truncated: declared {declared}, got {len(body)}"
        )
    return Packet.decode(body)
