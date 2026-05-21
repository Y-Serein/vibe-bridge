import json
import os
import tempfile
import unittest
from unittest import mock

from vibe_bridge.bootstrap import (
    ENV_HIDRAW_DEVICE,
    _daemon_ready,
    ensure_daemon_running,
    resolve_hidraw_device,
)
from vibe_bridge.transport_hidraw import HidrawDeviceInfo


class BootstrapTests(unittest.TestCase):
    def _write_state(self, state: dict) -> str:
        fd, path = tempfile.mkstemp(prefix="vibe-bridge-state-", suffix=".json")
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(state, f)
        self.addCleanup(lambda: os.path.exists(path) and os.unlink(path))
        return path

    def test_resolve_hidraw_device_uses_environment_override(self):
        with mock.patch.dict(os.environ, {ENV_HIDRAW_DEVICE: "/dev/custom"}, clear=True):
            self.assertEqual(resolve_hidraw_device(), "/dev/custom")

    def test_resolve_hidraw_device_prefers_vibe_vid_pid(self):
        devices = [
            HidrawDeviceInfo(path="/dev/hidraw0", vid=0x1234, pid=0x5678, readable=True, writable=True),
            HidrawDeviceInfo(path="/dev/hidraw1", vid=0x359F, pid=0x2120, readable=True, writable=True),
        ]
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch(
            "vibe_bridge.transport_hidraw.list_hidraw_devices", return_value=devices
        ):
            self.assertEqual(resolve_hidraw_device(), "/dev/hidraw1")

    def test_resolve_hidraw_device_ignores_non_vibe_single_rw_device(self):
        devices = [
            HidrawDeviceInfo(path="/dev/hidraw3", vid=None, pid=None, readable=True, writable=True),
        ]
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch(
            "vibe_bridge.transport_hidraw.list_hidraw_devices", return_value=devices
        ):
            self.assertIsNone(resolve_hidraw_device())

    def test_daemon_ready_accepts_mock_only_without_hidraw_device(self):
        state_path = self._write_state(
            {
                "sock_path": "/tmp/vibe-bridge.sock",
                "mode": "mock",
                "hidraw_path": None,
            }
        )
        with mock.patch("vibe_bridge.bootstrap.can_connect", return_value=True):
            self.assertTrue(_daemon_ready("/tmp/vibe-bridge.sock", None, state_path))

    def test_daemon_ready_rejects_mock_when_hidraw_device_exists(self):
        state_path = self._write_state(
            {
                "sock_path": "/tmp/vibe-bridge.sock",
                "mode": "mock",
                "hidraw_path": None,
            }
        )
        with mock.patch("vibe_bridge.bootstrap.can_connect", return_value=True):
            self.assertFalse(
                _daemon_ready("/tmp/vibe-bridge.sock", "/dev/hidraw0", state_path)
            )

    def test_daemon_ready_leaves_custom_socket_policy_to_caller(self):
        state_path = self._write_state(
            {
                "sock_path": "/tmp/custom-vibe.sock",
                "mode": "mock",
                "hidraw_path": None,
            }
        )
        with mock.patch("vibe_bridge.bootstrap.can_connect", return_value=True):
            self.assertTrue(
                _daemon_ready("/tmp/custom-vibe.sock", "/dev/hidraw0", state_path)
            )

    def test_daemon_ready_accepts_matching_real_hidraw_state(self):
        state_path = self._write_state(
            {
                "sock_path": "/tmp/vibe-bridge.sock",
                "mode": "real-hidraw",
                "hidraw_path": "/dev/hidraw0",
            }
        )
        with mock.patch("vibe_bridge.bootstrap.can_connect", return_value=True):
            self.assertTrue(
                _daemon_ready("/tmp/vibe-bridge.sock", "/dev/hidraw0", state_path)
            )

    def test_ensure_daemon_replaces_connectable_mock_when_hidraw_exists(self):
        state_path = self._write_state(
            {
                "sock_path": "/tmp/vibe-bridge.sock",
                "mode": "mock",
                "hidraw_path": None,
            }
        )

        def fake_spawn(sock_path, *, log_path, extra_args):
            self.assertEqual(sock_path, "/tmp/vibe-bridge.sock")
            self.assertEqual(
                extra_args,
                ["--state", state_path, "--hidraw", "/dev/hidraw0"],
            )
            with open(state_path, "w", encoding="utf-8") as f:
                json.dump(
                    {
                        "sock_path": "/tmp/vibe-bridge.sock",
                        "mode": "real-hidraw",
                        "hidraw_path": "/dev/hidraw0",
                    },
                    f,
                )
            return 1234

        with mock.patch("vibe_bridge.bootstrap.can_connect", return_value=True), mock.patch(
            "vibe_bridge.bootstrap.resolve_hidraw_device", return_value="/dev/hidraw0"
        ), mock.patch(
            "vibe_bridge.bootstrap.spawn_daemon_detached", side_effect=fake_spawn
        ) as spawn:
            self.assertFalse(
                ensure_daemon_running(
                    "/tmp/vibe-bridge.sock",
                    state_path=state_path,
                    timeout=0.1,
                    poll_interval=0.001,
                )
            )
            spawn.assert_not_called()

    def test_ensure_daemon_reuses_connectable_mock_without_hidraw(self):
        state_path = self._write_state(
            {
                "sock_path": "/tmp/vibe-bridge.sock",
                "mode": "mock",
                "hidraw_path": None,
            }
        )
        with mock.patch("vibe_bridge.bootstrap.can_connect", return_value=True), mock.patch(
            "vibe_bridge.bootstrap.resolve_hidraw_device", return_value=None
        ), mock.patch("vibe_bridge.bootstrap.spawn_daemon_detached") as spawn:
            self.assertTrue(
                ensure_daemon_running("/tmp/vibe-bridge.sock", state_path=state_path)
            )
            spawn.assert_not_called()


if __name__ == "__main__":
    unittest.main()
