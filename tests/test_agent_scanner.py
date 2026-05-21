from __future__ import annotations

import os
import tempfile
import threading
import time
import unittest
from typing import List, Optional

from vibe_bridge.agent_scanner import AgentScanner
from vibe_bridge.daemon import Daemon, DaemonConfig
from vibe_bridge.hid_protocol import (
    Cmd,
    Packet,
    SessionState,
    Status,
    make_session_response,
)
from vibe_bridge.mock_hid import MockHidClient


def _tmp_path(suffix: str) -> str:
    fd, path = tempfile.mkstemp(prefix="vibe-scanner-", suffix=suffix)
    os.close(fd)
    os.unlink(path)
    return path


def _wait_until(predicate, *, timeout: float = 2.0, interval: float = 0.01) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        time.sleep(interval)
    return predicate()


def _make_fake_proc(root: str, entries):
    """Build a tree of fake /proc/<pid>/cmdline and environ files.

    ``entries`` is an iterable of ``(pid, cmdline_args, environ_dict)``.
    """
    for pid, args, env in entries:
        pid_dir = os.path.join(root, str(pid))
        os.makedirs(pid_dir, exist_ok=True)
        with open(os.path.join(pid_dir, "cmdline"), "wb") as f:
            f.write(b"\x00".join(arg.encode() for arg in args) + b"\x00")
        with open(os.path.join(pid_dir, "environ"), "wb") as f:
            f.write(b"\x00".join(f"{k}={v}".encode() for k, v in env.items()) + b"\x00")
    # Also drop a non-numeric entry to make sure listdir filtering works.
    os.makedirs(os.path.join(root, "self"), exist_ok=True)


class AgentScannerPureTests(unittest.TestCase):
    """Tests that exercise the classify / enumerate logic on a fake /proc
    tree, without touching a real daemon socket."""

    def setUp(self) -> None:
        self.tmp = tempfile.mkdtemp(prefix="vibe-scanner-proc-")

    def tearDown(self) -> None:
        import shutil
        shutil.rmtree(self.tmp, ignore_errors=True)

    def _scanner(self) -> AgentScanner:
        return AgentScanner(sock_path="/nonexistent", proc_root=self.tmp)

    def test_classify_codex_basename(self) -> None:
        _make_fake_proc(self.tmp, [(101, ["/usr/local/bin/codex", "--help"], {})])
        live = self._scanner()._enumerate_agents()
        self.assertEqual(live, {101: "codex"})

    def test_classify_claude_code_basename(self) -> None:
        _make_fake_proc(self.tmp, [(202, ["/opt/claude-code", "/repo"], {})])
        live = self._scanner()._enumerate_agents()
        self.assertEqual(live, {202: "claude"})

    def test_excludes_wrapper_owned_processes(self) -> None:
        _make_fake_proc(self.tmp, [
            (303, ["/usr/local/bin/codex"], {"VIBE_SESSION_ID": "42"}),
            (404, ["/usr/local/bin/codex"], {"HOME": "/root"}),
        ])
        live = self._scanner()._enumerate_agents()
        self.assertEqual(live, {404: "codex"})

    def test_ignores_unrelated_processes(self) -> None:
        _make_fake_proc(self.tmp, [(505, ["/bin/bash", "-l"], {})])
        self.assertEqual(self._scanner()._enumerate_agents(), {})

    def test_ignores_empty_cmdline(self) -> None:
        pid_dir = os.path.join(self.tmp, "606")
        os.makedirs(pid_dir)
        # empty cmdline (kernel threads do this)
        open(os.path.join(pid_dir, "cmdline"), "wb").close()
        open(os.path.join(pid_dir, "environ"), "wb").close()
        self.assertEqual(self._scanner()._enumerate_agents(), {})


class AgentScannerWithDaemonTests(unittest.TestCase):
    """End-to-end: scanner sees a fake agent and goes through a full
    REQUEST_SESSION / SESSION_RESPONSE / STATUS_UPDATE round trip with the
    daemon."""

    def setUp(self) -> None:
        self.proc_root = tempfile.mkdtemp(prefix="vibe-scanner-proc-")
        self.sock_path = _tmp_path(".sock")
        self.state_path = _tmp_path(".json")
        self.screen_path = _tmp_path(".out")

        self.sent_lock = threading.Lock()
        self.sent: List[Packet] = []
        self.respond_with_sid: Optional[int] = None

        from queue import Queue
        from vibe_bridge.transport import Transport

        class FakeHid(Transport):
            def __init__(inner_self) -> None:
                inner_self._incoming: "Queue[object]" = Queue()
                inner_self.closed = False

            def send_packet(inner_self, packet: Packet) -> None:
                with self.sent_lock:
                    self.sent.append(packet)
                if (packet.command == int(Cmd.REQUEST_SESSION)
                        and self.respond_with_sid is not None):
                    inner_self._incoming.put(
                        make_session_response(self.respond_with_sid, Status.CREATED)
                    )

            def recv_packet(inner_self, timeout=None):
                from queue import Empty
                try:
                    item = inner_self._incoming.get(timeout=timeout)
                except Empty:
                    return None
                return item

            def close(inner_self) -> None:
                inner_self.closed = True
                inner_self._incoming.put(None)

        self.hid = FakeHid()
        cfg = DaemonConfig(
            sock_path=self.sock_path,
            state_path=self.state_path,
            screen_path=self.screen_path,
            hidraw_path="/dev/fake-hidraw-scanner",
            hid_transport=self.hid,
            reap_interval_seconds=999.0,
            heartbeat_interval_seconds=999.0,
            agent_scan_enabled=False,  # we start our own scanner with fake /proc
        )
        self.daemon = Daemon(cfg)
        self.daemon.start()

        self.scanner = AgentScanner(
            sock_path=self.sock_path,
            proc_root=self.proc_root,
            interval_seconds=999.0,  # we drive scan_once manually
        )

    def tearDown(self) -> None:
        self.scanner.stop()
        self.daemon.stop()
        import shutil
        shutil.rmtree(self.proc_root, ignore_errors=True)

    def test_first_scan_hooks_codex_with_status_update(self) -> None:
        _make_fake_proc(self.proc_root, [(701, ["/usr/local/bin/codex"], {})])
        self.respond_with_sid = 7

        self.scanner.scan_once()

        ok = _wait_until(lambda: any(p.command == int(Cmd.STATUS_UPDATE)
                                     and p.session_id == 7
                                     for p in self.sent))
        self.assertTrue(ok, f"expected STATUS_UPDATE for sid=7; sent={self.sent!r}")

        with self.sent_lock:
            req = [p for p in self.sent if p.command == int(Cmd.REQUEST_SESSION)]
            status = [p for p in self.sent if p.command == int(Cmd.STATUS_UPDATE)]
        self.assertEqual(len(req), 1)
        self.assertEqual(req[0].payload, b"codex")
        self.assertEqual(status[0].payload[:1], bytes([int(SessionState.RUN)]))

        # Hook table reflects the mapping.
        table = self.scanner.hook_table()
        self.assertEqual(table, [{"pid": 701, "kind": "codex", "sid": 7}])

    def test_second_scan_does_not_re_hook_same_pid(self) -> None:
        _make_fake_proc(self.proc_root, [(801, ["/usr/local/bin/codex"], {})])
        self.respond_with_sid = 8

        self.scanner.scan_once()
        _wait_until(lambda: self.scanner.hook_table())

        with self.sent_lock:
            before = sum(1 for p in self.sent
                         if p.command == int(Cmd.REQUEST_SESSION))

        # Second scan must not issue a new REQUEST_SESSION for the same PID.
        self.scanner.scan_once()
        time.sleep(0.05)

        with self.sent_lock:
            after = sum(1 for p in self.sent
                        if p.command == int(Cmd.REQUEST_SESSION))
        self.assertEqual(before, after)

    def test_disappearing_pid_unhooks_and_clears_table(self) -> None:
        _make_fake_proc(self.proc_root, [(901, ["/usr/local/bin/codex"], {})])
        self.respond_with_sid = 9

        self.scanner.scan_once()
        _wait_until(lambda: self.scanner.hook_table())

        # Remove the pid dir and rescan; hook table should empty out.
        import shutil
        shutil.rmtree(os.path.join(self.proc_root, "901"))
        self.scanner.scan_once()

        ok = _wait_until(lambda: self.scanner.hook_table() == [])
        self.assertTrue(ok, f"still hooked: {self.scanner.hook_table()}")


if __name__ == "__main__":
    unittest.main()
