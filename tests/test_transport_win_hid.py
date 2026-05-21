import unittest

from vibe_bridge.transport_win_hid import _extract_vid_pid


class WinHidTransportTests(unittest.TestCase):
    def test_extract_vid_pid_from_windows_hid_path(self):
        path = (
            r"\\?\hid#vid_359f&pid_2120&mi_00#8&abc&0&0000"
            r"#{4d1e55b2-f16f-11cf-88cb-001111000030}"
        )

        self.assertEqual(_extract_vid_pid(path), (0x359F, 0x2120))


if __name__ == "__main__":
    unittest.main()
