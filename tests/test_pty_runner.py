import os
import queue
import unittest

from vibe_bridge.pty_runner import _drain_injected_input


class PtyRunnerInjectedInputTests(unittest.TestCase):
    def test_drain_injected_input_writes_all_queued_bytes(self):
        read_fd, write_fd = os.pipe()
        injected = queue.Queue()
        try:
            injected.put(b"hello")
            injected.put(b" world")

            self.assertTrue(_drain_injected_input(write_fd, injected))

            os.close(write_fd)
            write_fd = -1
            self.assertEqual(os.read(read_fd, 64), b"hello world")
        finally:
            os.close(read_fd)
            if write_fd >= 0:
                os.close(write_fd)


if __name__ == "__main__":
    unittest.main()
