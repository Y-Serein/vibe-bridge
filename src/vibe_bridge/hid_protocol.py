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
    SESSION_HEARTBEAT = 0x04
    """Host -> board: keep-alive for ``sid``. Host emits one every 10s for every
    live session; board treats 30s of silence as DISCONNECTED."""
    SESSION_FOCUS = 0x05
    """Board -> host: user picked ``sid`` in the on-board grid. Host opens the
    VT100 stream gate so only ``sid``'s bytes are forwarded back to the board."""

    KEY_EVENT = 0x10
    ENCODER_EVENT = 0x11

    WINDOW_SWITCH = 0x20  # deprecated: board owns its own UI now
    WINDOW_ACTIVATE = 0x21  # deprecated: board owns its own UI now

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
    DISCONNECTED = 0x06


class SessionState(IntEnum):
    """1-byte ``state`` carried in ``CMD_STATUS_UPDATE`` payload[0].

    The board renders ``sid + state`` in the session grid. Heartbeat liveness
    overrides the value: 30s without a heartbeat forces ``DISCONNECTED``.
    """

    CONNECTED = 0x00
    DISCONNECTED = 0x01
    RUN = 0x02
    WAIT = 0x03
    DONE = 0x04
    ERROR = 0x05


class BoardKey(IntEnum):
    """AIKB physical key bit indexes in ``Cmd.KEY_EVENT`` payload byte 0."""

    REJECT = 0
    VOICE = 1
    SESSION = 2
    VOTE_REVIEW = 3
    AGENT_MODEL = 4
    MULTI_FUNCTION = 5
    CONFIRM = 6
    MENU_DEBUG = 7


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


@dataclass(frozen=True)
class KeyEvent:
    """Decoded board key bitmap.

    Payload schema, matching the current AIKB firmware:

    - byte 0: normal key bitmap, bits 0..6 map to current board keys;
      bit7 is protocol-compatible/reserved as :class:`BoardKey.MENU_DEBUG`
    - byte 1 bit 0: encoder push switch
    """

    key_bits: int
    encoder_pressed: bool

    def pressed(self, key: BoardKey) -> bool:
        return bool(self.key_bits & (1 << int(key)))


def decode_key_event_payload(payload: bytes) -> KeyEvent:
    if len(payload) < 2:
        raise ProtocolError(f"KEY_EVENT payload shorter than 2 bytes: {len(payload)}")
    return KeyEvent(
        key_bits=payload[0] & 0xFF,
        encoder_pressed=bool(payload[1] & 0x01),
    )


def encode_key_event_payload(key_bits: int, *, encoder_pressed: bool = False) -> bytes:
    if not 0 <= key_bits <= 0xFF:
        raise ProtocolError(f"key_bits out of range: {key_bits}")
    return bytes([key_bits & 0xFF, 0x01 if encoder_pressed else 0x00])


def decode_encoder_delta_payload(payload: bytes) -> int:
    if len(payload) < 1:
        raise ProtocolError("ENCODER_EVENT payload is empty")
    delta = payload[0]
    if delta > 127:
        delta -= 256
    return delta


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


def make_session_heartbeat(session_id: int) -> Packet:
    """Host -> board keepalive packet for ``session_id``."""
    return Packet(
        report_id=int(ReportId.DEVICE_BOUND),
        command=int(Cmd.SESSION_HEARTBEAT),
        session_id=session_id,
        payload=b"",
    )


def make_session_focus(session_id: int) -> Packet:
    """Board -> host focus packet. Used in tests; the firmware emits it natively."""
    return Packet(
        report_id=int(ReportId.HOST_BOUND),
        command=int(Cmd.SESSION_FOCUS),
        session_id=session_id,
        payload=b"",
    )


def make_status_update(session_id: int, state: SessionState) -> Packet:
    """Host -> board state report. Board mirrors the byte into the grid row."""
    return Packet(
        report_id=int(ReportId.DEVICE_BOUND),
        command=int(Cmd.STATUS_UPDATE),
        session_id=session_id,
        payload=bytes([int(state)]),
    )


def decode_status_update_payload(payload: bytes) -> SessionState:
    if not payload:
        raise ProtocolError("STATUS_UPDATE payload is empty")
    try:
        return SessionState(payload[0])
    except ValueError as exc:
        raise ProtocolError(f"unknown state byte 0x{payload[0]:02x}") from exc


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


def make_key_event(
    key_bits: int,
    *,
    encoder_pressed: bool = False,
    session_id: int = SESSION_BROADCAST,
) -> Packet:
    return Packet(
        report_id=int(ReportId.HOST_BOUND),
        command=int(Cmd.KEY_EVENT),
        session_id=session_id,
        payload=encode_key_event_payload(key_bits, encoder_pressed=encoder_pressed),
    )


def make_encoder_event(delta: int, *, session_id: int = SESSION_BROADCAST) -> Packet:
    if delta < -127:
        delta = -127
    if delta > 127:
        delta = 127
    return Packet(
        report_id=int(ReportId.HOST_BOUND),
        command=int(Cmd.ENCODER_EVENT),
        session_id=session_id,
        payload=bytes([(delta & 0xFF)]),
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
