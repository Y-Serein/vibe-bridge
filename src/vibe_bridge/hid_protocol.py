"""HID packet protocol for vibe-bridge.

Wire format (little-endian, see docs/hid_protocol.md):

    +--------+---------+-------------+-----------------+----------------+
    | u8     | u8      | u16         | u16             | bytes          |
    | report | command | session_id  | payload_length  | payload[len]   |
    +--------+---------+-------------+-----------------+----------------+

The header is fixed at 6 bytes. Real HID reports cap each frame at 64 bytes
(MAX_HID_REPORT_SIZE), so a single payload chunk holds up to MAX_PAYLOAD_PER_FRAME
bytes. Streams larger than that (typically CMD_VT100_STREAM) are fragmented by
the caller using ``fragment_payload``.

Mock transports (Unix sockets) accept arbitrary sizes; the size limits exist so
that the wire format stays compatible when we eventually swap in /dev/hidraw0.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass
from enum import IntEnum
from typing import Iterable, List

HEADER_FORMAT = "<BBHH"
HEADER_SIZE = struct.calcsize(HEADER_FORMAT)

MAX_HID_REPORT_SIZE = 64
MAX_PAYLOAD_PER_FRAME = MAX_HID_REPORT_SIZE - HEADER_SIZE

SESSION_BROADCAST = 0x0000


class ReportId(IntEnum):
    """HID report IDs. Names mirror the existing aikb_hid_input firmware."""

    HOST_BOUND = 0x10  # board -> host (key/encoder events, ACKs, session replies)
    DEVICE_BOUND = 0x20  # host -> board (VT100 stream, screen control, session req)
    ACK = 0x21  # board -> host, transport-level ack (legacy, optional)
    FEATURE = 0x30


class Cmd(IntEnum):
    """High-level commands carried in the ``command`` byte."""

    REQUEST_SESSION = 0x01
    SESSION_RESPONSE = 0x02
    SESSION_INVALID = 0x03

    KEY_EVENT = 0x10
    ENCODER_EVENT = 0x11

    WINDOW_SWITCH = 0x20
    WINDOW_ACTIVATE = 0x21

    VT100_STREAM = 0x30

    UI_SCALE_CHANGE = 0x40
    STATUS_UPDATE = 0x50
    FEEDBACK_EVENT = 0x60

    ERROR = 0xFF


class Status(IntEnum):
    """Status codes carried in CMD_SESSION_RESPONSE / CMD_SESSION_INVALID payloads."""

    OK = 0x00
    CREATED = 0x01
    INVALID = 0x02
    EXPIRED = 0x03
    POOL_FULL = 0x04
    RECLAIMED = 0x05


class ProtocolError(Exception):
    """Raised when a packet cannot be parsed."""


@dataclass(frozen=True)
class Packet:
    """A decoded HID packet."""

    report_id: int
    command: int
    session_id: int
    payload: bytes

    def encode(self) -> bytes:
        if not 0 <= self.session_id <= 0xFFFF:
            raise ProtocolError(f"session_id out of range: {self.session_id}")
        if len(self.payload) > 0xFFFF:
            raise ProtocolError(f"payload too long: {len(self.payload)}")
        header = struct.pack(
            HEADER_FORMAT,
            self.report_id & 0xFF,
            self.command & 0xFF,
            self.session_id & 0xFFFF,
            len(self.payload) & 0xFFFF,
        )
        return header + self.payload

    @classmethod
    def decode(cls, buf: bytes) -> "Packet":
        if len(buf) < HEADER_SIZE:
            raise ProtocolError(f"buffer shorter than header: {len(buf)} < {HEADER_SIZE}")
        report_id, command, sid, plen = struct.unpack(HEADER_FORMAT, buf[:HEADER_SIZE])
        payload = buf[HEADER_SIZE : HEADER_SIZE + plen]
        if len(payload) != plen:
            raise ProtocolError(
                f"payload truncated: declared {plen}, got {len(payload)}"
            )
        return cls(report_id=report_id, command=command, session_id=sid, payload=payload)

    def cmd(self) -> Cmd:
        return Cmd(self.command)


def make_request_session(
    *, report_id: int = ReportId.DEVICE_BOUND, hint: bytes = b""
) -> Packet:
    """Plugin -> daemon: ask for a fresh session_id.

    ``hint`` is an optional UTF-8 plugin name / wrapper hint that the session
    manager may stash for diagnostics. Caller owns the encoding.
    """
    return Packet(
        report_id=int(report_id),
        command=int(Cmd.REQUEST_SESSION),
        session_id=SESSION_BROADCAST,
        payload=hint,
    )


def make_session_response(session_id: int, status: Status) -> Packet:
    return Packet(
        report_id=int(ReportId.HOST_BOUND),
        command=int(Cmd.SESSION_RESPONSE),
        session_id=session_id,
        payload=bytes([int(status)]),
    )


def make_session_invalid(session_id: int, status: Status) -> Packet:
    return Packet(
        report_id=int(ReportId.HOST_BOUND),
        command=int(Cmd.SESSION_INVALID),
        session_id=session_id,
        payload=bytes([int(status)]),
    )


def make_vt100_chunk(session_id: int, chunk: bytes) -> Packet:
    return Packet(
        report_id=int(ReportId.DEVICE_BOUND),
        command=int(Cmd.VT100_STREAM),
        session_id=session_id,
        payload=chunk,
    )


def make_error(session_id: int, message: str) -> Packet:
    return Packet(
        report_id=int(ReportId.HOST_BOUND),
        command=int(Cmd.ERROR),
        session_id=session_id,
        payload=message.encode("utf-8")[:MAX_PAYLOAD_PER_FRAME],
    )


def fragment_payload(payload: bytes, *, chunk: int = MAX_PAYLOAD_PER_FRAME) -> List[bytes]:
    """Split a payload into HID-frame-sized chunks.

    Used by ``CMD_VT100_STREAM`` so each fragment fits a real 64-byte HID report.
    """
    if chunk <= 0:
        raise ValueError("chunk size must be positive")
    if not payload:
        return [b""]
    return [payload[i : i + chunk] for i in range(0, len(payload), chunk)]


def stream_iter_packets(session_id: int, payload: bytes) -> Iterable[Packet]:
    """Yield CMD_VT100_STREAM packets, fragmented to MAX_PAYLOAD_PER_FRAME."""
    for chunk in fragment_payload(payload):
        yield make_vt100_chunk(session_id, chunk)
