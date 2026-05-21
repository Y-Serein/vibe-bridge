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
    BoardKey,
    Cmd,
    Packet,
    ReportId,
    SessionState,
    Status,
    make_encoder_event,
    make_key_event,
    make_request_session,
    make_session_focus,
    make_session_response,
    make_status_update,
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

    def clear_sent(self) -> None:
        with self._sent_lock:
            self._sent.clear()


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
            heartbeat_interval_seconds=0.05,
            agent_scan_enabled=False,
        )
        self.daemon = Daemon(cfg)
        self.daemon.start()

    def tearDown(self) -> None:
        self.daemon.stop()

    def _wait_for_sent(self, count: int) -> None:
        ok = _wait_until(lambda: len(self.hid.sent()) >= count)
        self.assertTrue(ok, f"expected {count} hid writes, got {self.hid.sent()!r}")

    def _wait_for_command(self, command: Cmd) -> None:
        ok = _wait_until(
            lambda: any(p.command == int(command) for p in self.hid.sent())
        )
        self.assertTrue(ok, f"expected command {command}, got {self.hid.sent()!r}")

    def _vt100_packets(self, *, sid: Optional[int] = None) -> List[Packet]:
        packets = [p for p in self.hid.sent() if p.command == int(Cmd.VT100_STREAM)]
        if sid is not None:
            packets = [p for p in packets if p.session_id == sid]
        return packets

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
        self.assertFalse(
            any(p.command == int(Cmd.REQUEST_SESSION) for p in self.hid.sent())
        )
        self.assertFalse(
            any(p.command == int(Cmd.WINDOW_ACTIVATE) for p in self.hid.sent())
        )
        self.assertFalse(
            any(p.command == int(Cmd.VT100_STREAM) for p in self.hid.sent())
        )
        client = MockHidClient(self.sock_path)
        try:
            self._acquire_session(client, b"plugin-A", 42)
            self.assertEqual(self.daemon.sessions.get(42).plugin, "plugin-A")
        finally:
            client.close()

    def test_same_client_new_session_releases_old_sid(self) -> None:
        client = MockHidClient(self.sock_path)
        try:
            self._acquire_session(client, b"codex", 11)
            self._acquire_session(client, b"codex", 12)
            self.assertIsNone(self.daemon.sessions.get(11))
            self.assertIsNotNone(self.daemon.sessions.get(12))
        finally:
            client.close()

    # ----- new contract: board owns UI, host owns session lifecycle ---------

    def _no_dashboard_json(self) -> None:
        """No legacy panel JSON should ever be pushed in the new model."""
        for p in self.hid.sent():
            if p.command != int(Cmd.VT100_STREAM):
                continue
            self.assertFalse(
                p.payload.startswith(b'{"type":"'),
                f"unexpected dashboard JSON pushed by host: {p.payload!r}",
            )

    def test_no_dashboard_panel_pushed_on_any_board_key(self) -> None:
        client = MockHidClient(self.sock_path)
        try:
            self._acquire_session(client, b"codex", 11)
            self.hid.clear_sent()
            for key in (BoardKey.SESSION, BoardKey.VOICE, BoardKey.AGENT_MODEL,
                        BoardKey.VOTE_REVIEW, BoardKey.MULTI_FUNCTION,
                        BoardKey.CONFIRM, BoardKey.MENU_DEBUG, BoardKey.REJECT):
                self.hid.inject(make_key_event(1 << key))
            self.hid.inject(make_encoder_event(1))
            self.hid.inject(make_encoder_event(-1))
            self.hid.inject(make_key_event(0, encoder_pressed=True))
            time.sleep(0.05)
            self._no_dashboard_json()
            self.assertFalse(any(
                p.command in (int(Cmd.WINDOW_ACTIVATE), int(Cmd.WINDOW_SWITCH))
                for p in self.hid.sent()
            ))
        finally:
            client.close()

    def test_session_focus_from_board_opens_vt100_gate_and_replays(self) -> None:
        c1 = MockHidClient(self.sock_path)
        c2 = MockHidClient(self.sock_path)
        try:
            self._acquire_session(c1, b"plugin-A", 11)
            self._acquire_session(c2, b"plugin-B", 12)
            c1.send_packet(make_vt100_chunk(11, b"AAA"))
            c2.send_packet(make_vt100_chunk(12, b"BBB"))
            self.assertTrue(_wait_until(lambda: self.daemon.router.snapshot(11) == b"AAA"))
            self.assertTrue(_wait_until(lambda: self.daemon.router.snapshot(12) == b"BBB"))

            # With no focus, host must not forward any VT100 to the board.
            time.sleep(0.05)
            self.assertEqual(self._vt100_packets(), [])

            # Board confirms sid=12. Host opens the gate and replays its buffer.
            self.hid.inject(make_session_focus(12))
            self.assertTrue(_wait_until(lambda: self.daemon._focused_sid == 12))
            self.assertTrue(_wait_until(lambda: any(
                p.session_id == 12 and p.payload == SCREEN_CLEAR + b"BBB"
                for p in self._vt100_packets()
            )))
            # sid=11's bytes never crossed because it was never focused.
            self.assertFalse(any(
                p.session_id == 11 for p in self._vt100_packets()
            ))

            # Switching focus to sid=11 replays that buffer too.
            self.hid.inject(make_session_focus(11))
            self.assertTrue(_wait_until(lambda: self.daemon._focused_sid == 11))
            self.assertTrue(_wait_until(lambda: any(
                p.session_id == 11 and p.payload == SCREEN_CLEAR + b"AAA"
                for p in self._vt100_packets()
            )))
        finally:
            c1.close()
            c2.close()

    def test_session_focus_for_unknown_sid_is_ignored(self) -> None:
        self.hid.inject(make_session_focus(99))
        time.sleep(0.05)
        self.assertIsNone(self.daemon._focused_sid)

    def test_key_event_routes_to_sid_owner(self) -> None:
        c1 = MockHidClient(self.sock_path)
        c2 = MockHidClient(self.sock_path)
        try:
            self._acquire_session(c1, b"plugin-A", 11)
            self._acquire_session(c2, b"plugin-B", 12)
            # Drain the session-response replies so the next recv is the event.
            c1.recv_packet(timeout=0.5)
            c2.recv_packet(timeout=0.5)

            self.hid.inject(make_key_event(1 << BoardKey.CONFIRM, session_id=12))
            pkt = c2.recv_packet(timeout=1.0)
            self.assertIsNotNone(pkt)
            self.assertEqual(pkt.command, int(Cmd.KEY_EVENT))
            self.assertEqual(pkt.session_id, 12)
            # The other client should not see anything.
            self.assertIsNone(c1.recv_packet(timeout=0.05))
        finally:
            c1.close()
            c2.close()

    def test_encoder_event_routes_to_sid_owner(self) -> None:
        c1 = MockHidClient(self.sock_path)
        c2 = MockHidClient(self.sock_path)
        try:
            self._acquire_session(c1, b"plugin-A", 11)
            self._acquire_session(c2, b"plugin-B", 12)
            c1.recv_packet(timeout=0.5)
            c2.recv_packet(timeout=0.5)

            self.hid.inject(make_encoder_event(-1, session_id=11))
            pkt = c1.recv_packet(timeout=1.0)
            self.assertIsNotNone(pkt)
            self.assertEqual(pkt.command, int(Cmd.ENCODER_EVENT))
            self.assertEqual(pkt.session_id, 11)
            self.assertIsNone(c2.recv_packet(timeout=0.05))
        finally:
            c1.close()
            c2.close()

    def test_key_event_with_sid_zero_is_dropped(self) -> None:
        client = MockHidClient(self.sock_path)
        try:
            self._acquire_session(client, b"codex", 11)
            client.recv_packet(timeout=0.5)
            self.hid.inject(make_key_event(1 << BoardKey.CONFIRM))  # default sid=0
            self.assertIsNone(client.recv_packet(timeout=0.1))
        finally:
            client.close()

    def test_window_cmds_from_plugin_are_silently_dropped(self) -> None:
        client = MockHidClient(self.sock_path)
        try:
            self._acquire_session(client, b"codex", 11)
            client.recv_packet(timeout=0.5)
            self.hid.clear_sent()
            for cmd in (Cmd.WINDOW_ACTIVATE, Cmd.WINDOW_SWITCH):
                client.send_packet(Packet(
                    report_id=int(ReportId.HOST_BOUND),
                    command=int(cmd),
                    session_id=11,
                    payload=b"",
                ))
            time.sleep(0.05)
            self.assertFalse(any(
                p.command in (int(Cmd.WINDOW_ACTIVATE), int(Cmd.WINDOW_SWITCH))
                for p in self.hid.sent()
            ))
        finally:
            client.close()

    def test_heartbeat_thread_emits_one_packet_per_live_sid(self) -> None:
        c1 = MockHidClient(self.sock_path)
        c2 = MockHidClient(self.sock_path)
        try:
            self._acquire_session(c1, b"codex", 11)
            self._acquire_session(c2, b"codex-2", 12)
            self.hid.clear_sent()
            self.assertTrue(_wait_until(
                lambda: {
                    p.session_id for p in self.hid.sent()
                    if p.command == int(Cmd.SESSION_HEARTBEAT)
                } >= {11, 12},
                timeout=2.0,
            ))
        finally:
            c1.close()
            c2.close()

    def test_status_update_from_plugin_is_forwarded_to_board(self) -> None:
        client = MockHidClient(self.sock_path)
        try:
            self._acquire_session(client, b"codex", 11)
            self.hid.clear_sent()
            client.send_packet(make_status_update(11, SessionState.RUN))
            self.assertTrue(_wait_until(lambda: any(
                p.command == int(Cmd.STATUS_UPDATE)
                and p.session_id == 11
                and p.payload[:1] == bytes([int(SessionState.RUN)])
                for p in self.hid.sent()
            )))
        finally:
            client.close()

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
            self.assertTrue(
                _wait_until(
                    lambda: any(
                        p.command == int(Cmd.UI_SCALE_CHANGE) for p in self.hid.sent()
                    )
                )
            )
            forwarded = [
                p for p in self.hid.sent() if p.command == int(Cmd.UI_SCALE_CHANGE)
            ]
            self.assertEqual(len(forwarded), 1)
            self.assertEqual(forwarded[0].session_id, 0)
            self.assertEqual(forwarded[0].payload, bytes([12, 24]))
        finally:
            client.close()

    def test_hid_screen_write_does_not_block_on_stalled_hid_send(self) -> None:
        entered = threading.Event()
        release = threading.Event()
        original_send = self.hid.send_packet
        # Simulate "board focused sid=11" so the gate is open for this test.
        self.daemon.router.register(11)
        self.daemon._focused_sid = 11

        def blocking_send(packet: Packet) -> None:
            entered.set()
            release.wait(timeout=5.0)
            original_send(packet)

        self.hid.send_packet = blocking_send  # type: ignore[method-assign]
        worker = threading.Thread(
            target=self.daemon._on_hid_screen_write,
            args=(11, b"stalled output"),
            daemon=True,
        )
        try:
            worker.start()
            worker.join(timeout=0.2)
            self.assertFalse(worker.is_alive(), "screen write must not block plugin handling")
            self.assertTrue(entered.wait(timeout=1.0), "hid writer should attempt the send")
        finally:
            release.set()
            worker.join(timeout=1.0)
            self.hid.send_packet = original_send  # type: ignore[method-assign]

    def test_hid_disconnect_marks_state_unavailable_for_bootstrap_recovery(self) -> None:
        with mock.patch("vibe_bridge.daemon.log"):
            self.hid.inject_error(OSError(errno.ENODEV, "device gone"))
            ok = _wait_until(lambda: self._read_state().get("mode") == "mock")
        self.assertTrue(ok)
        state = self._read_state()
        self.assertIsNone(state.get("hidraw_path"))


if __name__ == "__main__":
    unittest.main()
