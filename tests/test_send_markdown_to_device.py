import importlib.util
from pathlib import Path
import unittest


MODULE_PATH = Path(__file__).resolve().parents[1] / "scripts" / "send_markdown_to_device.py"
SPEC = importlib.util.spec_from_file_location("send_markdown_to_device", MODULE_PATH)
send_markdown = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(send_markdown)


class SendMarkdownToDeviceTests(unittest.TestCase):
    def test_pipe_table_is_rendered_as_aligned_text(self):
        data = b"| Kind | Result |\n| --- | --- |\n| table | text |\n"

        out = send_markdown.render_text_chunk(data, lambda target: b"")

        self.assertEqual(
            out,
            b"+-------+--------+\n"
            b"| Kind  | Result |\n"
            b"+-------+--------+\n"
            b"| table | text   |\n"
            b"+-------+--------+\n",
        )

    def test_code_fence_is_rendered_as_small_screen_text(self):
        data = b"```md\n| Kind | Result |\n| --- | --- |\n```\n"

        out = send_markdown.render_text_chunk(data, lambda target: b"")

        self.assertEqual(out, b"Code: md\n  | Kind | Result |\n  | --- | --- |\n")

    def test_markdown_image_still_uses_renderer(self):
        data = b"before\n![alt](./img.png)\nafter\n"

        out = send_markdown.render_text_chunk(data, lambda target: b"<image>\n")

        self.assertEqual(out, b"before\n<image>\nafter\n")

    def test_standalone_image_does_not_keep_source_line_newline(self):
        data = b"![alt](./img.png)\nafter\n"

        out = send_markdown.render_text_chunk(data, lambda target: b"<image>\r\n")

        self.assertEqual(out, b"<image>\r\nafter\n")

    def test_heading_is_rendered_as_small_screen_title(self):
        data = b"# Kitty MD LCD\n"

        out = send_markdown.render_text_chunk(data, lambda target: b"")

        self.assertEqual(out, b"== Kitty MD LCD ==\n")


if __name__ == "__main__":
    unittest.main()
