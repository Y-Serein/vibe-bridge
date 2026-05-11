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

        self.assertEqual(out, b"Kind   Result\ntable  text\n")

    def test_pipe_table_inside_code_fence_is_left_alone(self):
        data = b"```md\n| Kind | Result |\n| --- | --- |\n```\n"

        out = send_markdown.render_text_chunk(data, lambda target: b"")

        self.assertEqual(out, data)

    def test_markdown_image_still_uses_renderer(self):
        data = b"before\n![alt](./img.png)\nafter\n"

        out = send_markdown.render_text_chunk(data, lambda target: b"<image>")

        self.assertEqual(out, b"before\n<image>\nafter\n")


if __name__ == "__main__":
    unittest.main()
