"""End-to-end test: real Daemon + real Unix socket + two plugin clients.

We swap the daemon's screen sink for an in-memory recorder so we can assert on
exactly which (sid, bytes) tuples get mirrored to the LCD. This pins the
contract that:

* per-session VT100 buffers stay isolated;
* only the active session's bytes hit the screen sink on append;
* WINDOW_SWITCH and WINDOW_ACTIVATE replay the new session's buffer with a
  screen-clear so the LCD shows the new window's last state, not a frozen frame
  from the previous one.
"""

from __future__ import annotations

import os
import tempfile
import threading
import time
import unittest
from typing import List, Tuple

from vibe_bridge.daemon import Daemon, DaemonConfig
from vibe_bridge.hid_protocol import (
    Cmd,
    Packet,
    ReportId,
    SESSION_BROADCAST,
    Status,
    make_request_session,
    make_vt100_chunk,
)
from vibe_bridge.mock_hid import MockHidClient
from vibe_bridge.vt100_router import SCREEN_CLEAR


def _tmp_path(suffix: str) -> str:
    fd, path = tempfile.mkstemp(prefix="vibe-bridge-it-", suffix=suffix)
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


def _acquire_session(client: MockHidClient, hint: bytes) -> int:
    client.send_packet(make_request_session(hint=hint))
    pkt = client.recv_packet(timeout=1.0)
    assert pkt is not None, "no SESSION_RESPONSE"
    assert pkt.command == int(Cmd.SESSION_RESPONSE), pkt
    assert pkt.payload[0] == int(Status.CREATED)
    return pkt.session_id


class WindowSwitchIntegrationTests(unittest.TestCase):
    def setUp(self) -> None:
        sock_path = _tmp_path(".sock")
        state_path = _tmp_path(".json")
        screen_path = _tmp_path(".out")
        cfg = DaemonConfig(
            sock_path=sock_path,
            state_path=state_path,
            screen_path=screen_path,
            agent_scan_enabled=False,
        )
        self.daemon = Daemon(cfg)
        self.daemon.start()

        self._sink_lock = threading.Lock()
        self.sink_calls: List[Tuple[int, bytes]] = []

        def sink(sid: int, data: bytes) -> None:
            with self._sink_lock:
                self.sink_calls.append((sid, data))

        self.daemon.router.set_screen_sink(sink)
        self.sock_path = sock_path

    def tearDown(self) -> None:
        self.daemon.stop()

    # ------------------------------------------------------------ helpers

    def _sink_count(self) -> int:
        with self._sink_lock:
            return len(self.sink_calls)

    def _wait_for_sink(self, n: int, *, timeout: float = 1.0) -> None:
        ok = _wait_until(lambda: self._sink_count() >= n, timeout=timeout)
        with self._sink_lock:
            self.assertTrue(
                ok,
                f"sink only got {len(self.sink_calls)}/{n} calls: {self.sink_calls!r}",
            )

    # --------------------------------------------------------------- test

    def test_two_sessions_isolated_and_switched_by_router(self) -> None:
        """Board owns the picker now; in mock mode we drive
        ``router.set_active`` directly to simulate the effect of an inbound
        ``CMD_SESSION_FOCUS`` from the board firmware."""
        c1 = MockHidClient(self.sock_path)
        c2 = MockHidClient(self.sock_path)
        try:
            sid1 = _acquire_session(c1, b"plugin-A")
            sid2 = _acquire_session(c2, b"plugin-B")
            self.assertNotEqual(sid1, sid2)

            # sid1 registered first → it is the default active window.
            self.assertEqual(self.daemon.router.active(), sid1)
            c1.send_packet(make_vt100_chunk(sid1, b"AAA"))
            self._wait_for_sink(1)
            with self._sink_lock:
                self.assertEqual(self.sink_calls[-1], (sid1, b"AAA"))

            # Plugin B writes while sid2 is inactive → buffered, no sink call.
            c2.send_packet(make_vt100_chunk(sid2, b"BBB"))
            time.sleep(0.05)
            with self._sink_lock:
                self.assertEqual(len(self.sink_calls), 1, self.sink_calls)
            self.assertEqual(self.daemon.router.snapshot(sid2), b"BBB")

            # Board "focuses" sid2 → router replays buffer.
            self.assertTrue(self.daemon.router.set_active(sid2))
            self._wait_for_sink(2)
            with self._sink_lock:
                self.assertEqual(self.sink_calls[-1], (sid2, SCREEN_CLEAR + b"BBB"))

            # Live updates for sid2 now mirror immediately.
            c2.send_packet(make_vt100_chunk(sid2, b"CCC"))
            self._wait_for_sink(3)
            with self._sink_lock:
                self.assertEqual(self.sink_calls[-1], (sid2, b"CCC"))

            # Plugin A writes while inactive → buffered only.
            c1.send_packet(make_vt100_chunk(sid1, b"DDD"))
            time.sleep(0.05)
            with self._sink_lock:
                self.assertEqual(len(self.sink_calls), 3)
            self.assertEqual(self.daemon.router.snapshot(sid1), b"AAADDD")

            # Board re-focuses sid1 → replay AAA+DDD.
            self.assertTrue(self.daemon.router.set_active(sid1))
            self._wait_for_sink(4)
            with self._sink_lock:
                self.assertEqual(self.sink_calls[-1], (sid1, SCREEN_CLEAR + b"AAADDD"))
        finally:
            c1.close()
            c2.close()

    def test_window_switch_with_no_sessions_is_noop(self) -> None:
        # Connect a transient client purely to send the WINDOW_SWITCH.
        # No sessions exist; daemon should swallow the packet without crash.
        c = MockHidClient(self.sock_path)
        try:
            switch_pkt = Packet(
                report_id=int(ReportId.HOST_BOUND),
                command=int(Cmd.WINDOW_SWITCH),
                session_id=SESSION_BROADCAST,
                payload=bytes([1]),
            )
            c.send_packet(switch_pkt)
            # Give the daemon a moment.
            time.sleep(0.05)
            self.assertIsNone(self.daemon.router.active())
        finally:
            c.close()

    def test_active_session_disconnect_releases_and_replays_next_window(self) -> None:
        c1 = MockHidClient(self.sock_path)
        c2 = MockHidClient(self.sock_path)
        try:
            sid1 = _acquire_session(c1, b"plugin-A")
            sid2 = _acquire_session(c2, b"plugin-B")
            self.assertEqual(self.daemon.router.active(), sid1)

            c1.send_packet(make_vt100_chunk(sid1, b"AAA"))
            self._wait_for_sink(1)
            c2.send_packet(make_vt100_chunk(sid2, b"BBB"))
            time.sleep(0.05)

            c1.close()
            ok = _wait_until(
                lambda: self.daemon.sessions.get(sid1) is None
                and self.daemon.router.active() == sid2
            )
            self.assertTrue(ok)
            self._wait_for_sink(2)
            with self._sink_lock:
                self.assertEqual(self.sink_calls[-1], (sid2, SCREEN_CLEAR + b"BBB"))
        finally:
            c2.close()


if __name__ == "__main__":
    unittest.main()
