import unittest

from vibe_bridge.hid_protocol import (
    Cmd,
    HEADER_SIZE,
    MAX_PAYLOAD_PER_FRAME,
    Packet,
    ProtocolError,
    ReportId,
    Status,
    fragment_payload,
    make_request_session,
    make_session_response,
    make_vt100_chunk,
    stream_iter_packets,
)


class PacketRoundtripTests(unittest.TestCase):
    def test_empty_payload_roundtrip(self):
        pkt = Packet(report_id=0x10, command=int(Cmd.REQUEST_SESSION), session_id=0, payload=b"")
        raw = pkt.encode()
        self.assertEqual(len(raw), HEADER_SIZE)
        self.assertEqual(Packet.decode(raw), pkt)

    def test_with_payload_roundtrip(self):
        payload = bytes(range(50))
        pkt = Packet(
            report_id=int(ReportId.DEVICE_BOUND),
            command=int(Cmd.VT100_STREAM),
            session_id=0x1234,
            payload=payload,
        )
        raw = pkt.encode()
        self.assertEqual(len(raw), HEADER_SIZE + len(payload))
        decoded = Packet.decode(raw)
        self.assertEqual(decoded.session_id, 0x1234)
        self.assertEqual(decoded.payload, payload)
        self.assertIs(decoded.cmd(), Cmd.VT100_STREAM)

    def test_decode_truncated_raises(self):
        pkt = Packet(report_id=0x20, command=0x30, session_id=1, payload=b"hello")
        raw = pkt.encode()
        with self.assertRaises(ProtocolError):
            Packet.decode(raw[:-2])
        with self.assertRaises(ProtocolError):
            Packet.decode(b"\x00")

    def test_session_id_overflow_rejected(self):
        with self.assertRaises(ProtocolError):
            Packet(report_id=0, command=0, session_id=0x10000, payload=b"").encode()


class HelpersTests(unittest.TestCase):
    def test_request_session_helper(self):
        req = make_request_session(hint=b"codex")
        self.assertIs(req.cmd(), Cmd.REQUEST_SESSION)
        self.assertEqual(req.session_id, 0)
        self.assertEqual(req.payload, b"codex")

    def test_session_response_helper(self):
        resp = make_session_response(7, Status.CREATED)
        self.assertIs(resp.cmd(), Cmd.SESSION_RESPONSE)
        self.assertEqual(resp.session_id, 7)
        self.assertEqual(resp.payload, bytes([int(Status.CREATED)]))

    def test_vt100_chunk_helper(self):
        chunk = make_vt100_chunk(7, b"hi")
        self.assertIs(chunk.cmd(), Cmd.VT100_STREAM)
        self.assertEqual(chunk.payload, b"hi")


class FragmentationTests(unittest.TestCase):
    def test_fragment_respects_hid_frame_size(self):
        payload = b"x" * (MAX_PAYLOAD_PER_FRAME * 3 + 5)
        chunks = fragment_payload(payload)
        self.assertEqual(len(chunks), 4)
        for c in chunks:
            self.assertLessEqual(len(c), MAX_PAYLOAD_PER_FRAME)
        self.assertEqual(b"".join(chunks), payload)

    def test_fragment_empty_yields_single_empty(self):
        self.assertEqual(fragment_payload(b""), [b""])

    def test_stream_iter_packets_carries_session_id(self):
        payload = b"a" * (MAX_PAYLOAD_PER_FRAME + 10)
        pkts = list(stream_iter_packets(42, payload))
        self.assertEqual(len(pkts), 2)
        for pkt in pkts:
            self.assertEqual(pkt.session_id, 42)
            self.assertIs(pkt.cmd(), Cmd.VT100_STREAM)
        self.assertEqual(pkts[0].payload + pkts[1].payload, payload)


if __name__ == "__main__":
    unittest.main()
