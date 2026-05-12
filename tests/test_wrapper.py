import os
import unittest
from unittest import mock

from vibe_bridge.mock_hid import DEFAULT_SOCK_PATH
from vibe_bridge.wrapper import (
    DEFAULT_LCD_COLS,
    DEFAULT_LCD_ROWS,
    ENV_LCD_COLS,
    ENV_LCD_CHAR_ADAPT,
    ENV_LCD_ROWS,
    ENV_LCD_THEME,
    ENV_REUSE_SESSION,
    ENV_SESSION_ID,
    ENV_SOCK_PATH,
    LcdOutputAdapter,
    LEGACY_REAL_SOCK_PATH,
    _existing_session_from_env,
    _legacy_real_sock_should_fall_back,
    _lcd_output_adapter_from_env,
    _lcd_pty_size,
    _open_session_client,
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

    def test_resolve_sock_path_ignores_stale_legacy_mock_when_default_is_real(self):
        def fake_state(path):
            if path == "/tmp/vibe-real-state.json":
                return {"sock_path": LEGACY_REAL_SOCK_PATH, "mode": "mock"}
            if path == "/tmp/vibe-bridge-state.json":
                return {
                    "sock_path": DEFAULT_SOCK_PATH,
                    "mode": "real-hidraw",
                    "hidraw_path": "/dev/hidraw0",
                }
            return None

        with mock.patch.dict(os.environ, {ENV_SOCK_PATH: LEGACY_REAL_SOCK_PATH}), mock.patch(
            "vibe_bridge.wrapper._load_daemon_state", side_effect=fake_state
        ), mock.patch("vibe_bridge.wrapper.can_connect", return_value=True):
            self.assertEqual(_resolve_sock_path(None), DEFAULT_SOCK_PATH)

    def test_resolve_sock_path_keeps_legacy_socket_when_it_is_real(self):
        with mock.patch.dict(os.environ, {ENV_SOCK_PATH: LEGACY_REAL_SOCK_PATH}), mock.patch(
            "vibe_bridge.wrapper._load_daemon_state",
            return_value={"mode": "real-hidraw", "hidraw_path": "/dev/hidraw0"},
        ), mock.patch("vibe_bridge.wrapper.can_connect", return_value=True):
            self.assertEqual(_resolve_sock_path(None), LEGACY_REAL_SOCK_PATH)

    def test_resolve_sock_path_ignores_dead_legacy_socket(self):
        with mock.patch.dict(os.environ, {ENV_SOCK_PATH: LEGACY_REAL_SOCK_PATH}), mock.patch(
            "vibe_bridge.wrapper.can_connect", return_value=False
        ):
            self.assertEqual(_resolve_sock_path(None), DEFAULT_SOCK_PATH)

    def test_legacy_real_sock_falls_back_when_hidraw_exists(self):
        with mock.patch(
            "vibe_bridge.wrapper._load_daemon_state",
            return_value={"mode": "mock", "hidraw_path": None},
        ), mock.patch("vibe_bridge.wrapper.can_connect", return_value=False), mock.patch(
            "vibe_bridge.wrapper.resolve_hidraw_device", return_value="/dev/hidraw0"
        ):
            self.assertTrue(_legacy_real_sock_should_fall_back(LEGACY_REAL_SOCK_PATH))

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

    def test_open_session_activates_new_sid(self):
        sent = []

        class FakePlugin:
            def __init__(self, *, plugin_name, sock_path):
                self.plugin_name = plugin_name
                self.sock_path = sock_path

            def connect(self):
                pass

            def acquire_session(self, timeout):
                return 77

            def send_packet(self, packet):
                sent.append(packet)

        with mock.patch(
            "vibe_bridge.wrapper.ensure_daemon_running", return_value=True
        ), mock.patch("vibe_bridge.wrapper.PluginClient", FakePlugin):
            plugin, sid = _open_session_client("codex", DEFAULT_SOCK_PATH)

        self.assertIsNotNone(plugin)
        self.assertEqual(sid, 77)
        self.assertEqual(len(sent), 1)
        self.assertEqual(sent[0].session_id, 77)
        self.assertEqual(sent[0].command, 0x21)

    def test_open_session_activates_reused_sid(self):
        sent = []

        class FakePlugin:
            def __init__(self, *, plugin_name, sock_path):
                self.sid = None

            def adopt_session(self, sid):
                self.sid = sid

            def connect(self):
                pass

            def send_packet(self, packet):
                sent.append(packet)

        with mock.patch.dict(
            os.environ,
            {ENV_SESSION_ID: "88", ENV_REUSE_SESSION: "1"},
            clear=True,
        ), mock.patch(
            "vibe_bridge.wrapper.ensure_daemon_running", return_value=True
        ), mock.patch("vibe_bridge.wrapper.PluginClient", FakePlugin):
            plugin, sid = _open_session_client("codex", DEFAULT_SOCK_PATH)

        self.assertIsNotNone(plugin)
        self.assertEqual(sid, 88)
        self.assertEqual(len(sent), 1)
        self.assertEqual(sent[0].session_id, 88)
        self.assertEqual(sent[0].command, 0x21)

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

    def test_lcd_output_adapter_is_on_by_default(self):
        with mock.patch.dict(os.environ, {}, clear=True):
            self.assertIsInstance(_lcd_output_adapter_from_env(), LcdOutputAdapter)

    def test_lcd_output_adapter_can_be_disabled_for_raw_forwarding(self):
        with mock.patch.dict(os.environ, {ENV_LCD_CHAR_ADAPT: "0"}, clear=True):
            self.assertIsNone(_lcd_output_adapter_from_env())

    def test_lcd_output_adapter_requires_explicit_opt_in(self):
        with mock.patch.dict(os.environ, {ENV_LCD_CHAR_ADAPT: "1"}, clear=True):
            self.assertIsInstance(_lcd_output_adapter_from_env(), LcdOutputAdapter)

    def test_lcd_output_adapter_theme_is_opt_in(self):
        with mock.patch.dict(
            os.environ,
            {ENV_LCD_CHAR_ADAPT: "1", ENV_LCD_THEME: "gruvbox"},
            clear=True,
        ):
            adapter = _lcd_output_adapter_from_env()
        self.assertIsNotNone(adapter)
        if adapter is None:
            self.fail("adapter should be enabled")
        self.assertIn(
            "\x1b[38;2;235;219;178m",
            adapter.feed(b"hello\n").decode(),
        )

    def test_lcd_output_adapter_replaces_tui_symbols(self):
        adapter = LcdOutputAdapter()
        self.assertEqual(
            adapter.feed("⏺ Claude ╭─╮ ✓ ❯ ▪ ■\n".encode()),
            "· Claude ╭─╮ v > · ·\n".encode(),
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

        self.assertIn("序号  名称    数量  备注", out)
        self.assertIn("1     项目 A  10    已完成", out)
        self.assertEqual(out.count("\n"), text.count("\n"))
        self.assertNotIn("+------+--------+------+--------+", out)
        self.assertNotIn("|---:|---|---:|---|", out)

    def test_lcd_output_adapter_does_not_markdown_render_user_prompt(self):
        adapter = LcdOutputAdapter()
        text = (
            "> | 序号 | 名称 |\n"
            "> |---|---|\n"
            "> | 1 | 用户输入 |\n"
        )

        out = adapter.feed(text.encode()).decode()

        self.assertIn("> | 序号 | 名称 |", out)
        self.assertIn("> |---|---|", out)
        self.assertNotIn("序号  名称", out)

    def test_lcd_output_adapter_does_not_markdown_render_system_lines(self):
        adapter = LcdOutputAdapter()
        text = (
            "Tip: | A | B |\n"
            "|---|---|\n"
            "| 1 | 2 |\n"
        )

        out = adapter.feed(text.encode()).decode()

        self.assertIn("Tip: | A | B |", out)
        self.assertIn("|---|---|", out)

    def test_lcd_output_adapter_detects_colored_markdown_table(self):
        adapter = LcdOutputAdapter()
        text = (
            "\x1b[1m· | 序号 | 项目 | 内容 | 备注 |\x1b[0m\n"
            "\x1b[2m|---:|---|---|---|\x1b[0m\n"
            "| 1 | A | B | C |\n"
            "\n"
        )

        out = adapter.feed(text.encode()).decode()

        self.assertIn("序号  项目  内容  备注", out)
        self.assertIn("1     A     B     C", out)
        self.assertNotIn("|---:|---|---|---|", out)

    def test_lcd_output_adapter_detects_pipe_only_separator(self):
        adapter = LcdOutputAdapter()
        text = "| A | B | C |\n||||\n| 1 | 2 | 3 |\n\n"

        out = adapter.feed(text.encode()).decode()

        self.assertIn("A  B  C", out)
        self.assertIn("1  2  3", out)
        self.assertNotIn("||||", out)

    def test_lcd_output_adapter_detects_positioned_markdown_table(self):
        adapter = LcdOutputAdapter()
        text = (
            "\x1b[4;1H| 序号 | 项目 | 状态 |\n"
            "\x1b[5;1H|---:|---|---|\n"
            "\x1b[6;1H| 1 | 需求整理 | 未开始 |\n"
            "\n"
        )

        out = adapter.feed(text.encode()).decode()

        self.assertIn("序号  项目      状态", out)
        self.assertIn("1     需求整理  未开始", out)
        self.assertNotIn("|---:|---|---|", out)
        self.assertEqual(out.count("\n"), text.count("\n"))

    def test_lcd_output_adapter_holds_partial_table_line(self):
        adapter = LcdOutputAdapter()

        self.assertEqual(adapter.feed(b"| A | B |"), b"")
        out = adapter.feed(b"\n|---|---|\n| 1 | 2 |\n\n").decode()

        self.assertIn("A  B", out)
        self.assertIn("1  2", out)

    def test_lcd_output_adapter_keeps_table_open_across_chunks(self):
        adapter = LcdOutputAdapter()

        self.assertEqual(adapter.feed(b"| A | B |\n|---|---|\n"), b"")
        out = adapter.feed(b"| 1 | 2 |\n\n").decode()

        self.assertIn("A  B", out)
        self.assertIn("1  2", out)

    def test_lcd_output_adapter_converts_codex_bulleted_markdown_table(self):
        adapter = LcdOutputAdapter()
        text = (
            "\x1b[2m.\x1b[0m | 编号 | 姓名 | 部门 | 职位 | 入职日期 | 状态 |\n"
            "  |---|---|---|---|---|---|\n"
            "  | 001 | 张三 | 技术部 | 后端工程师 | 2024-03-12 | 在职 |\n"
            "  | 002 | 李四 | 产品部 | 产品经理 | 2023-11-05 | 在职 |\n"
            "\n"
        )

        out = adapter.feed(text.encode()).decode()

        self.assertIn("编号  姓名  部门", out)
        self.assertIn("001", out)
        self.assertIn("张三", out)
        self.assertIn("后端工程师", out)
        self.assertNotIn("|---|---|---|---|---|---|", out)

    def test_lcd_output_adapter_removes_reverse_video_without_reflow(self):
        adapter = LcdOutputAdapter(theme="gruvbox")

        out = adapter.feed(b"\x1b[7m> Explain this codebase\x1b[0m\n").decode()

        self.assertIn("> Explain this codebase", out)
        self.assertNotIn("\x1b[7m", out)
        self.assertNotIn("|> Explain", out)
        self.assertNotIn("+---------", out)

    def test_lcd_output_adapter_maps_reverse_video_to_visible_input_box(self):
        adapter = LcdOutputAdapter()

        out = adapter.feed(b"\x1b[7m> hello\x1b[0m\n").decode()

        self.assertIn("\x1b[38;2;249;245;215;48;2;80;73;69m> hello", out)
        self.assertNotIn("\x1b[7m", out)

    def test_lcd_output_adapter_constrains_256_color_background_to_dark_gruvbox(self):
        adapter = LcdOutputAdapter(theme="gruvbox")

        out = adapter.feed(b"\x1b[48;5;250mbar\x1b[0m\n").decode()

        self.assertIn("bar", out)
        self.assertNotIn("\x1b[48;5;250m", out)
        self.assertNotIn("\x1b[48;2;213;196;161m", out)

    def test_lcd_output_adapter_preserves_terminal_controls(self):
        adapter = LcdOutputAdapter()

        out = adapter.feed(b"answer\x1b[12;30Hprompt\x1b[K\n").decode()

        self.assertEqual(out, "answer\x1b[12;30Hprompt\x1b[K\n")

    def test_lcd_output_adapter_preserves_codex_tui_structure(self):
        adapter = LcdOutputAdapter(theme="gruvbox")
        text = (
            "\x1b[7mWorking (5s · esc to interrupt)\x1b[0m\n"
            "\x1b[7m> Write tests for @filename\x1b[0m\n"
            "\x1b[48;5;4mgpt-5.5 high · ~/AIKB\x1b[0m\n"
        )

        out = adapter.feed(text.encode()).decode()

        self.assertIn("> Write tests for @filename", out)
        self.assertNotIn("|> Write tests", out)
        self.assertNotIn("+----------------", out)
        self.assertIn("Working (5s · esc to interrupt)", out)
        self.assertIn("gpt-5.5 high · ~/AIKB", out)

    def test_lcd_output_adapter_flushes_non_table_pipe_lines(self):
        adapter = LcdOutputAdapter()

        out = adapter.feed(b"| just | text |\nnot a separator\n").decode()

        self.assertEqual(out, "| just | text |\nnot a separator\n")

    def test_lcd_output_adapter_applies_gruvbox_default_theme(self):
        adapter = LcdOutputAdapter(theme="gruvbox")

        out = adapter.feed(b"hello\n").decode()

        self.assertTrue(out.startswith("\x1b[38;2;235;219;178m\x1b[48;2;40;40;40m"))
        self.assertIn("hello\n", out)

    def test_lcd_output_adapter_remaps_ansi_color_to_gruvbox(self):
        adapter = LcdOutputAdapter(theme="gruvbox")

        out = adapter.feed(b"\x1b[31merror\x1b[0m\n").decode()

        self.assertIn("\x1b[38;2;204;36;29m", out)
        self.assertIn("error", out)
        self.assertIn("\x1b[38;2;235;219;178m\x1b[48;2;40;40;40m", out)

    def test_lcd_output_adapter_constrains_truecolor_to_gruvbox_palette(self):
        adapter = LcdOutputAdapter(theme="gruvbox")

        out = adapter.feed(b"\x1b[48;2;1;2;3mboxed\x1b[0m\n").decode()

        self.assertIn("\x1b[48;2;60;56;54mboxed", out)
        self.assertNotIn("48;2;48;2", out)


if __name__ == "__main__":
    unittest.main()
