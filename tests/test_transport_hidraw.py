"""Tests for ``HidrawTransport`` packet codec, padding and framing.

We can't open a real ``/dev/hidraw0`` in CI, but the transport's encode-pad
behaviour is testable in isolation by swapping the underlying fd for a Unix
``socketpair``: the kernel preserves message boundaries on ``SOCK_SEQPACKET``,
which is the closest stdlib analogue to per-report HID framing.
"""

from __future__ import annotations

import errno
import os
import socket
import unittest
from unittest import mock

from vibe_bridge.hid_protocol import (
    Cmd,
    Packet,
    ReportId,
    Status,
    make_request_session,
    make_session_response,
)
from vibe_bridge.transport_hidraw import (
    DEFAULT_REPORT_LENGTH,
    HEADER_SIZE,
    HidrawTransport,
    list_hidraw_devices,
)


class HidrawCodecTests(unittest.TestCase):
    """Drive HidrawTransport against a SEQPACKET socketpair instead of a real /dev/hidraw."""

    def setUp(self) -> None:
        a, b = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        self._a = a
        self._b = b
        self.transport = HidrawTransport.__new__(HidrawTransport)
        # Bypass __init__ (which calls os.open) and inject our socket fd.
        self.transport._fd = a.fileno()
        self.transport._device_path = "<test>"
        self.transport._report_length = DEFAULT_REPORT_LENGTH
        import threading

        self.transport._lock = threading.Lock()

    def tearDown(self) -> None:
        self._a.close()
        self._b.close()

    def test_send_pads_to_report_length(self):
        pkt = make_request_session(hint=b"abc")
        self.transport.send_packet(pkt)
        raw = self._b.recv(DEFAULT_REPORT_LENGTH)
        self.assertEqual(len(raw), DEFAULT_REPORT_LENGTH)
        # body bytes match Packet.encode()
        body_len = HEADER_SIZE + len(pkt.payload)
        self.assertEqual(raw[:body_len], pkt.encode())
        # padding tail is zero-filled
        self.assertTrue(all(b == 0 for b in raw[body_len:]))

    def test_recv_decodes_padded_frame(self):
        pkt = make_session_response(7, Status.CREATED)
        framed = pkt.encode() + b"\x00" * (DEFAULT_REPORT_LENGTH - len(pkt.encode()))
        self._b.send(framed)
        decoded = self.transport.recv_packet(timeout=1.0)
        self.assertIsNotNone(decoded)
        self.assertEqual(decoded.session_id, 7)
        self.assertEqual(decoded.cmd(), Cmd.SESSION_RESPONSE)
        # Padding past payload_length is ignored.
        self.assertEqual(decoded.payload, bytes([int(Status.CREATED)]))

    def test_send_rejects_oversize(self):
        oversized = Packet(
            report_id=int(ReportId.DEVICE_BOUND),
            command=int(Cmd.VT100_STREAM),
            session_id=1,
            payload=b"x" * (DEFAULT_REPORT_LENGTH),
        )
        with self.assertRaises(Exception):
            self.transport.send_packet(oversized)

    def test_recv_timeout_returns_none(self):
        self.assertIsNone(self.transport.recv_packet(timeout=0.05))

    def test_recv_short_frame_raises(self):
        # Send fewer bytes than HEADER_SIZE → decode should refuse.
        self._b.send(b"\x10\x01")
        with self.assertRaises(Exception):
            self.transport.recv_packet(timeout=0.5)


class HidrawListTests(unittest.TestCase):
    def test_list_returns_empty_when_no_devices(self):
        with mock.patch("vibe_bridge.transport_hidraw.glob", return_value=[]):
            self.assertEqual(list_hidraw_devices(), [])


if __name__ == "__main__":
    unittest.main()
