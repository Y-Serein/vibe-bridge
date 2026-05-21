import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from argparse import Namespace
from unittest import mock

from vibe_bridge.main import _describe_last_board_event, cmd_doctor, cmd_sessions, main
from vibe_bridge.transport_hidraw import HidrawDeviceInfo


class MainCliTests(unittest.TestCase):
    def test_describe_last_board_event_decodes_key_bits(self):
        self.assertEqual(
            _describe_last_board_event("key bits=0x04 enc=0"),
            "SESSION",
        )
        self.assertEqual(
            _describe_last_board_event("key bits=0x00 enc=1"),
            "ENCODER_PRESS",
        )
        self.assertEqual(
            _describe_last_board_event("key bits=0x02 enc=0"),
            "VOICE",
        )

    def test_sessions_reports_socket_liveness_and_state_age(self):
        fd, path = tempfile.mkstemp(prefix="vibe-bridge-state-", suffix=".json")
        self.addCleanup(lambda: os.path.exists(path) and os.unlink(path))
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(
                {
                    "sock_path": "/tmp/vibe-bridge.sock",
                    "mode": "real-hidraw",
                    "hidraw_path": "/dev/hidraw0",
                    "focused_sid": 33,
                    "hooked_agents": [{"pid": 701, "kind": "codex", "sid": 33}],
                    "sessions": [],
                    "buffers": {},
                },
                f,
            )

        out = io.StringIO()
        with mock.patch("vibe_bridge.main.can_connect", return_value=False):
            with contextlib.redirect_stdout(out):
                rc = cmd_sessions(Namespace(state=path, sock="/tmp/fallback.sock"))

        self.assertEqual(rc, 0)
        text = out.getvalue()
        self.assertIn("daemon socket : /tmp/vibe-bridge.sock", text)
        self.assertIn("socket status : unreachable/stale", text)
        self.assertIn("state age     :", text)
        self.assertIn("focused sid   : 33", text)
        self.assertIn("hooked agents : 1", text)
        self.assertIn("pid=701  kind=codex  sid=33", text)

    def test_doctor_ignores_non_vibe_hidraw(self):
        fd, path = tempfile.mkstemp(prefix="vibe-bridge-state-", suffix=".json")
        self.addCleanup(lambda: os.path.exists(path) and os.unlink(path))
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump({"sock_path": "/tmp/vibe-bridge.sock", "mode": "mock"}, f)

        devices = [
            HidrawDeviceInfo(
                path="/dev/hidraw0",
                vid=0x1234,
                pid=0x5678,
                readable=True,
                writable=True,
            )
        ]
        out = io.StringIO()
        with mock.patch("vibe_bridge.main._probe_socket", return_value=(False, "connection refused")), \
                mock.patch("vibe_bridge.transport_hidraw.list_hidraw_devices", return_value=devices), \
                mock.patch("vibe_bridge.main.resolve_hidraw_device", return_value=None), \
                mock.patch("vibe_bridge.main.shutil.which", return_value=None), \
                mock.patch("vibe_bridge.main.os.path.exists", return_value=True):
            with contextlib.redirect_stdout(out):
                rc = cmd_doctor(
                    Namespace(
                        sock="/tmp/vibe-bridge.sock",
                        state=path,
                        log="/tmp/vibe-bridge-daemon.log",
                        cli=[],
                        color="auto",
                    )
                )

        self.assertEqual(rc, 0)
        text = out.getvalue()
        self.assertIn("no Vibe HID device (359f:2120) detected", text)
        self.assertIn("non-Vibe hidraw devices ignored by auto-selection: 1", text)
        self.assertIn("automatic hidraw selection is disabled", text)

    def test_doctor_reports_vibe_auto_selection(self):
        fd, path = tempfile.mkstemp(prefix="vibe-bridge-state-", suffix=".json")
        self.addCleanup(lambda: os.path.exists(path) and os.unlink(path))
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump({"sock_path": "/tmp/vibe-bridge.sock", "mode": "mock"}, f)

        devices = [
            HidrawDeviceInfo(
                path="/dev/hidraw2",
                vid=0x359F,
                pid=0x2120,
                readable=True,
                writable=True,
            )
        ]
        out = io.StringIO()
        with mock.patch("vibe_bridge.main._probe_socket", return_value=(True, None)), \
                mock.patch("vibe_bridge.transport_hidraw.list_hidraw_devices", return_value=devices), \
                mock.patch("vibe_bridge.main.resolve_hidraw_device", return_value="/dev/hidraw2"), \
                mock.patch("vibe_bridge.main.shutil.which", return_value=None), \
                mock.patch("vibe_bridge.main.os.path.exists", return_value=True):
            with contextlib.redirect_stdout(out):
                rc = cmd_doctor(
                    Namespace(
                        sock="/tmp/vibe-bridge.sock",
                        state=path,
                        log="/tmp/vibe-bridge-daemon.log",
                        cli=[],
                        color="auto",
                    )
                )

        self.assertEqual(rc, 0)
        text = out.getvalue()
        self.assertIn("Vibe HID 359f:2120 at /dev/hidraw2 permissions=rw", text)
        self.assertIn("automatic hidraw selection: /dev/hidraw2", text)

    def test_doctor_color_always_colorizes_status_labels(self):
        fd, path = tempfile.mkstemp(prefix="vibe-bridge-state-", suffix=".json")
        self.addCleanup(lambda: os.path.exists(path) and os.unlink(path))
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump({"sock_path": "/tmp/vibe-bridge.sock", "mode": "mock"}, f)

        out = io.StringIO()
        with mock.patch("vibe_bridge.main._probe_socket", return_value=(False, "connection refused")), \
                mock.patch("vibe_bridge.transport_hidraw.list_hidraw_devices", return_value=[]), \
                mock.patch("vibe_bridge.main.resolve_hidraw_device", return_value=None), \
                mock.patch("vibe_bridge.main.shutil.which", return_value=None), \
                mock.patch("vibe_bridge.main.os.path.exists", return_value=True):
            with contextlib.redirect_stdout(out):
                rc = cmd_doctor(
                    Namespace(
                        sock="/tmp/vibe-bridge.sock",
                        state=path,
                        log="/tmp/vibe-bridge-daemon.log",
                        cli=[],
                        color="always",
                    )
                )

        self.assertEqual(rc, 0)
        self.assertIn("\033[33m[WARN]\033[0m", out.getvalue())

    def test_windows_doctor_fails_fast_outside_native_windows(self):
        out = io.StringIO()
        with mock.patch("vibe_bridge.windows_host.platform.system", return_value="Linux"):
            with contextlib.redirect_stdout(out):
                rc = main(["windows", "doctor"])

        self.assertEqual(rc, 1)
        text = out.getvalue()
        self.assertIn("not running on native Windows", text)
        self.assertIn("native Windows HID transport is implemented", text)

    def test_windows_plan_prints_required_adapters(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            rc = main(["windows", "plan"])

        self.assertEqual(rc, 0)
        text = out.getvalue()
        self.assertIn("windows-cli", text)
        self.assertIn("wsl-cli", text)
        self.assertIn("vscode", text)
        self.assertIn("browser", text)
        self.assertIn("WINDOW_ACTIVATE", text)

    def test_repo_root_python_m_works_without_pythonpath(self):
        repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
        env = os.environ.copy()
        env.pop("PYTHONPATH", None)

        proc = subprocess.run(
            [sys.executable, "-m", "vibe_bridge.main", "windows", "plan"],
            cwd=repo,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=5,
        )

        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("vibe-bridge windows product plan", proc.stdout)

    def test_windows_cli_dispatches_runner(self):
        with mock.patch("vibe_bridge.windows_runner.run_windows_cli", return_value=0) as run:
            rc = main(["windows", "cli", "--ipc", "tcp://127.0.0.1:9000", "--", "codex"])

        self.assertEqual(rc, 0)
        run.assert_called_once_with(
            ["--", "codex"],
            sock_path="tcp://127.0.0.1:9000",
            plugin_name=None,
        )

    def test_windows_wsl_cli_dispatches_runner(self):
        with mock.patch("vibe_bridge.windows_runner.run_wsl_cli", return_value=0) as run:
            rc = main(
                [
                    "windows",
                    "wsl-cli",
                    "--ipc",
                    "tcp://127.0.0.1:9000",
                    "--distro",
                    "Ubuntu",
                    "--wsl-cwd",
                    "~",
                    "--",
                    "codex",
                ]
            )

        self.assertEqual(rc, 0)
        run.assert_called_once_with(
            ["--", "codex"],
            sock_path="tcp://127.0.0.1:9000",
            plugin_name="wsl-cli",
            distro="Ubuntu",
            wsl_cwd="~",
        )


if __name__ == "__main__":
    unittest.main()
