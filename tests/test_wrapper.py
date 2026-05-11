import os
import unittest
from unittest import mock

from vibe_bridge.mock_hid import DEFAULT_SOCK_PATH
from vibe_bridge.wrapper import (
    DEFAULT_LCD_COLS,
    DEFAULT_LCD_ROWS,
    ENV_LCD_COLS,
    ENV_LCD_ROWS,
    ENV_REUSE_SESSION,
    ENV_SESSION_ID,
    ENV_SOCK_PATH,
    LcdOutputAdapter,
    LEGACY_REAL_SOCK_PATH,
    _existing_session_from_env,
    _lcd_pty_size,
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

    def test_lcd_pty_size_defaults_to_current_lcd_grid(self):
        with mock.patch.dict(os.environ, {}, clear=True):
            self.assertEqual(_lcd_pty_size(), (DEFAULT_LCD_ROWS, DEFAULT_LCD_COLS))

    def test_lcd_pty_size_reads_environment(self):
        with mock.patch.dict(
            os.environ,
            {ENV_LCD_ROWS: "10", ENV_LCD_COLS: "40"},
            clear=True,
        ):
            self.assertEqual(_lcd_pty_size(), (10, 40))

    def test_lcd_output_adapter_replaces_tui_symbols(self):
        adapter = LcdOutputAdapter()
        self.assertEqual(
            adapter.feed("⏺ Claude ╭─╮ ✓ ❯\n".encode()),
            "· Claude +-+ v >\n".encode(),
        )

    def test_lcd_output_adapter_handles_split_utf8(self):
        adapter = LcdOutputAdapter()
        data = "前⏺后".encode()
        self.assertEqual(adapter.feed(data[:4]), "前".encode())
        self.assertEqual(adapter.feed(data[4:]), "·后".encode())

    def test_lcd_output_adapter_renders_live_markdown_table(self):
        adapter = LcdOutputAdapter()
        text = (
            "  | 序号 | 名称 | 数量 | 备注 |\n"
            "  |---:|---|---:|---|\n"
            "  | 1 | 项目 A | 10 | 已完成 |\n"
            "  | 2 | 项目 B | 5 | 进行中 |\n"
            "\n"
        )

        out = adapter.feed(text.encode()).decode()

        self.assertIn("+------+--------+------+--------+", out)
        self.assertIn("| 序号 | 名称   | 数量 | 备注   |", out)
        self.assertNotIn("|---:|---|---:|---|", out)

    def test_lcd_output_adapter_flushes_non_table_pipe_lines(self):
        adapter = LcdOutputAdapter()

        out = adapter.feed(b"| just | text |\nnot a separator\n").decode()

        self.assertEqual(out, "| just | text |\nnot a separator\n")


if __name__ == "__main__":
    unittest.main()
