import os
import unittest
from unittest import mock

from vibe_bridge.mock_hid import DEFAULT_SOCK_PATH
from vibe_bridge.wrapper import (
    ENV_REUSE_SESSION,
    ENV_SESSION_ID,
    ENV_SOCK_PATH,
    LEGACY_REAL_SOCK_PATH,
    _existing_session_from_env,
    _resolve_sock_path,
)


class WrapperTests(unittest.TestCase):
    def test_resolve_sock_path_defaults_to_standard_socket(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch(
            "vibe_bridge.wrapper.can_connect", return_value=False
        ):
            self.assertEqual(_resolve_sock_path(None), DEFAULT_SOCK_PATH)

    def test_resolve_sock_path_reads_environment(self):
        with mock.patch.dict(os.environ, {ENV_SOCK_PATH: "/tmp/custom.sock"}):
            self.assertEqual(_resolve_sock_path(None), "/tmp/custom.sock")

    def test_explicit_sock_path_wins_over_environment(self):
        with mock.patch.dict(os.environ, {ENV_SOCK_PATH: "/tmp/custom.sock"}):
            self.assertEqual(_resolve_sock_path("/tmp/explicit.sock"), "/tmp/explicit.sock")

    def test_resolve_sock_path_reuses_legacy_real_socket(self):
        def fake_can_connect(path):
            return path == LEGACY_REAL_SOCK_PATH

        with mock.patch.dict(os.environ, {}, clear=True), mock.patch(
            "vibe_bridge.wrapper.can_connect", side_effect=fake_can_connect
        ):
            self.assertEqual(_resolve_sock_path(None), LEGACY_REAL_SOCK_PATH)

    def test_session_id_is_not_reused_by_default(self):
        with mock.patch.dict(os.environ, {ENV_SESSION_ID: "42"}, clear=True):
            self.assertIsNone(_existing_session_from_env())

    def test_session_id_reuse_requires_explicit_opt_in(self):
        with mock.patch.dict(
            os.environ,
            {ENV_SESSION_ID: "42", ENV_REUSE_SESSION: "1"},
            clear=True,
        ):
            self.assertEqual(_existing_session_from_env(), 42)


if __name__ == "__main__":
    unittest.main()
