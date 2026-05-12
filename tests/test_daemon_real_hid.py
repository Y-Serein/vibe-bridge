from __future__ import annotations

import errno
import json
import os
import queue
import tempfile
import threading
import time
import unittest
from typing import List, Optional
from unittest import mock

from vibe_bridge.daemon import Daemon, DaemonConfig
from vibe_bridge.hid_protocol import (
    Cmd,
    Packet,
    ReportId,
    Status,
    make_request_session,
    make_session_response,
    make_vt100_chunk,
)
from vibe_bridge.mock_hid import MockHidClient
from vibe_bridge.transport import Transport
from vibe_bridge.vt100_router import SCREEN_CLEAR


def _tmp_path(suffix: str) -> str:
    fd, path = tempfile.mkstemp(prefix="vibe-bridge-real-", suffix=suffix)
    os.close(fd)
    os.unlink(path)
    return path


def _wait_until(predicate, *, timeout: float = 1.0, interval: float = 0.01) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        time.sleep(interval)
    return predicate()


class FakeHidTransport(Transport):
    def __init__(self) -> None:
        self._incoming: "queue.Queue[object]" = queue.Queue()
        self._sent: List[Packet] = []
        self._sent_lock = threading.Lock()
        self.closed = False

    def send_packet(self, packet: Packet) -> None:
        with self._sent_lock:
            self._sent.append(packet)

    def recv_packet(self, timeout: Optional[float] = None) -> Optional[Packet]:
        try:
            item = self._incoming.get(timeout=timeout)
        except queue.Empty:
            return None
        if isinstance(item, BaseException):
            raise item
        return item

    def close(self) -> None:
        self.closed = True
        self._incoming.put(None)

    def inject(self, packet: Packet) -> None:
        self._incoming.put(packet)

    def inject_error(self, exc: BaseException) -> None:
        self._incoming.put(exc)

    def sent(self) -> List[Packet]:
        with self._sent_lock:
            return list(self._sent)


class RealHidDaemonTests(unittest.TestCase):
    def setUp(self) -> None:
        self.sock_path = _tmp_path(".sock")
        self.state_path = _tmp_path(".json")
        self.screen_path = _tmp_path(".out")
        self.hid = FakeHidTransport()
        cfg = DaemonConfig(
            sock_path=self.sock_path,
            state_path=self.state_path,
            screen_path=self.screen_path,
            hidraw_path="/dev/fake-hidraw0",
            hid_transport=self.hid,
            reap_interval_seconds=999.0,
        )
        self.daemon = Daemon(cfg)
        self.daemon.start()

    def tearDown(self) -> None:
        self.daemon.stop()

    def _wait_for_sent(self, count: int) -> None:
        ok = _wait_until(lambda: len(self.hid.sent()) >= count)
        self.assertTrue(ok, f"expected {count} hid writes, got {self.hid.sent()!r}")

    def _read_state(self) -> dict:
        with open(self.state_path, "r", encoding="utf-8") as f:
            return json.load(f)

    def _acquire_session(self, client: MockHidClient, hint: bytes, sid: int) -> Packet:
        before = len(self.hid.sent())
        client.send_packet(make_request_session(hint=hint))
        self._wait_for_sent(before + 1)
        forwarded = self.hid.sent()[-1]
        self.assertEqual(forwarded.command, int(Cmd.REQUEST_SESSION))
        self.assertEqual(forwarded.payload, hint)
        self.hid.inject(make_session_response(sid, Status.CREATED))
        reply = client.recv_packet(timeout=1.0)
        self.assertIsNotNone(reply)
        self.assertEqual(reply.command, int(Cmd.SESSION_RESPONSE))
        self.assertEqual(reply.session_id, sid)
        return reply

    def test_start_does_not_request_session_until_plugin_request(self) -> None:
        self.assertEqual(self.hid.sent(), [])
        client = MockHidClient(self.sock_path)
        try:
            self._acquire_session(client, b"plugin-A", 42)
            self.assertEqual(self.daemon.sessions.get(42).plugin, "plugin-A")
        finally:
            client.close()

    def test_vt100_uses_local_router_and_board_assigned_sid(self) -> None:
        c1 = MockHidClient(self.sock_path)
        c2 = MockHidClient(self.sock_path)
        try:
            self._acquire_session(c1, b"plugin-A", 11)
            self._acquire_session(c2, b"plugin-B", 12)

            c1.send_packet(make_vt100_chunk(11, b"AAA"))
            self._wait_for_sent(3)
            vt100 = [p for p in self.hid.sent() if p.command == int(Cmd.VT100_STREAM)]
            self.assertEqual(vt100[-1].session_id, 11)
            self.assertEqual(vt100[-1].payload, b"AAA")

            c2.send_packet(make_vt100_chunk(12, b"BBB"))
            time.sleep(0.05)
            vt100 = [p for p in self.hid.sent() if p.command == int(Cmd.VT100_STREAM)]
            self.assertEqual(len(vt100), 1)
            self.assertEqual(self.daemon.router.snapshot(12), b"BBB")

            encoder_next = Packet(
                report_id=int(ReportId.HOST_BOUND),
                command=int(Cmd.ENCODER_EVENT),
                session_id=0,
                payload=bytes([1]),
            )
            self.hid.inject(encoder_next)
            ok = _wait_until(lambda: self.daemon.router.active() == 12)
            self.assertTrue(ok)
            self._wait_for_sent(4)
            vt100 = [p for p in self.hid.sent() if p.command == int(Cmd.VT100_STREAM)]
            self.assertEqual(vt100[-1].session_id, 12)
            self.assertEqual(vt100[-1].payload, SCREEN_CLEAR + b"BBB")
        finally:
            c1.close()
            c2.close()

    def test_ui_scale_change_is_forwarded_to_board(self) -> None:
        client = MockHidClient(self.sock_path)
        try:
            scale_pkt = Packet(
                report_id=int(ReportId.HOST_BOUND),
                command=int(Cmd.UI_SCALE_CHANGE),
                session_id=0,
                payload=bytes([12, 24]),
            )
            client.send_packet(scale_pkt)
            self._wait_for_sent(1)
            forwarded = [
                p for p in self.hid.sent() if p.command == int(Cmd.UI_SCALE_CHANGE)
            ]
            self.assertEqual(len(forwarded), 1)
            self.assertEqual(forwarded[0].session_id, 0)
            self.assertEqual(forwarded[0].payload, bytes([12, 24]))
        finally:
            client.close()

    def test_hid_disconnect_marks_state_unavailable_for_bootstrap_recovery(self) -> None:
        with mock.patch("vibe_bridge.daemon.log"):
            self.hid.inject_error(OSError(errno.ENODEV, "device gone"))
            ok = _wait_until(lambda: self._read_state().get("mode") == "mock")
        self.assertTrue(ok)
        state = self._read_state()
        self.assertIsNone(state.get("hidraw_path"))


if __name__ == "__main__":
    unittest.main()
