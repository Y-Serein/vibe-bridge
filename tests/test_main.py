import contextlib
import io
import json
import os
import tempfile
import unittest
from argparse import Namespace
from unittest import mock

from vibe_bridge.main import cmd_sessions


class MainCliTests(unittest.TestCase):
    def test_sessions_reports_socket_liveness_and_state_age(self):
        fd, path = tempfile.mkstemp(prefix="vibe-bridge-state-", suffix=".json")
        self.addCleanup(lambda: os.path.exists(path) and os.unlink(path))
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(
                {
                    "sock_path": "/tmp/vibe-bridge.sock",
                    "mode": "real-hidraw",
                    "hidraw_path": "/dev/hidraw0",
                    "active_sid": 33,
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


if __name__ == "__main__":
    unittest.main()
