import unittest
from unittest import mock

from vibe_bridge.windows_runner import (
    CTRL_C_EXIT_CODE,
    _command_line,
    _env_block,
    _wait_for_process,
    build_wsl_command,
    run_windows_cli,
)


class WindowsRunnerTests(unittest.TestCase):
    def test_command_line_quotes_arguments(self):
        self.assertEqual(
            _command_line(["codex.exe", "hello world", "--flag"]),
            'codex.exe "hello world" --flag',
        )

    def test_env_block_sorts_and_terminates(self):
        block = _env_block({"b": "2", "A": "1", "bad=key": "skip"})

        self.assertEqual(block, "A=1\0b=2\0\0")

    def test_build_wsl_command_uses_windows_host_launcher(self):
        self.assertEqual(
            build_wsl_command(["--", "codex", "--model", "gpt"], distro="Ubuntu", cwd="~"),
            ["wsl.exe", "-d", "Ubuntu", "--cd", "~", "--", "codex", "--model", "gpt"],
        )

    def test_windows_cli_does_not_activate_board_on_session_create(self):
        sent = []
        captured = {}

        class FakePlugin:
            def __init__(self, **kwargs):
                self.kwargs = kwargs

            def connect(self):
                pass

            def acquire_session(self, timeout):
                return 55

            def send_packet(self, packet):
                sent.append(packet)

            def send_vt100(self, data):
                pass

            def set_board_packet_handler(self, callback):
                pass

            def close(self):
                pass

        with mock.patch("vibe_bridge.windows_runner.platform.system", return_value="Windows"), \
                mock.patch("vibe_bridge.windows_runner.shutil.which", return_value="C:\\bin\\codex.exe"), \
                mock.patch("vibe_bridge.windows_runner.PluginClient", FakePlugin), \
                mock.patch("vibe_bridge.windows_runner._run_conpty", side_effect=lambda argv, **kwargs: captured.update(kwargs) or 0):
            rc = run_windows_cli(["codex"], sock_path="tcp://127.0.0.1:8765")

        self.assertEqual(rc, 0)
        self.assertEqual(sent, [])
        self.assertEqual(captured["env"]["VIBE_BRIDGE_DISABLE"], "1")
        self.assertEqual(captured["env"]["VIBE_SESSION_ID"], "55")

    def test_wait_for_process_terminates_child_on_ctrl_c(self):
        calls = []

        class FakeKernel32:
            def WaitForSingleObject(self, process_handle, timeout_ms):
                calls.append(("wait", process_handle, timeout_ms))
                raise KeyboardInterrupt

            def TerminateProcess(self, process_handle, exit_code):
                calls.append(("terminate", process_handle, exit_code))
                return True

        rc = _wait_for_process(FakeKernel32(), 123)

        self.assertEqual(rc, CTRL_C_EXIT_CODE)
        self.assertEqual(calls, [
            ("wait", 123, 100),
            ("terminate", 123, CTRL_C_EXIT_CODE),
        ])


if __name__ == "__main__":
    unittest.main()
