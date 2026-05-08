import os
import unittest
from unittest import mock

from vibe_bridge.bootstrap import ENV_HIDRAW_DEVICE, resolve_hidraw_device
from vibe_bridge.transport_hidraw import HidrawDeviceInfo


class BootstrapTests(unittest.TestCase):
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

    def test_resolve_hidraw_device_falls_back_to_single_rw_device(self):
        devices = [
            HidrawDeviceInfo(path="/dev/hidraw3", vid=None, pid=None, readable=True, writable=True),
        ]
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch(
            "vibe_bridge.transport_hidraw.list_hidraw_devices", return_value=devices
        ):
            self.assertEqual(resolve_hidraw_device(), "/dev/hidraw3")


if __name__ == "__main__":
    unittest.main()
