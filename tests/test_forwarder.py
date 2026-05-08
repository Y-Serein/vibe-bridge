import threading
import time
import unittest

from vibe_bridge.forwarder import Forwarder


class ForwarderTests(unittest.TestCase):
    def test_drains_to_send_callback(self):
        received = []
        cond = threading.Condition()

        def send(data: bytes) -> None:
            with cond:
                received.append(data)
                cond.notify_all()

        f = Forwarder(send)
        f.start()
        try:
            f.push(b"hello")
            f.push(b"world")
            with cond:
                deadline = time.time() + 1.0
                while len(received) < 2 and time.time() < deadline:
                    cond.wait(timeout=deadline - time.time())
            self.assertEqual(received, [b"hello", b"world"])
            stats = f.stats()
            self.assertEqual(stats.pushed, 2)
            self.assertEqual(stats.sent, 2)
            self.assertEqual(stats.dropped, 0)
        finally:
            f.stop()

    def test_overflow_drops_oldest(self):
        block_sender = threading.Event()
        unblock_sender = threading.Event()
        sent = []

        def send(data: bytes) -> None:
            block_sender.set()
            unblock_sender.wait(timeout=2.0)
            sent.append(data)

        f = Forwarder(send, max_queue=2)
        f.start()
        try:
            # Push enough to overflow while sender is blocked on first item.
            f.push(b"a")
            self.assertTrue(block_sender.wait(timeout=1.0))
            f.push(b"b")
            f.push(b"c")
            f.push(b"d")  # overflow -> drops one
            f.push(b"e")  # overflow -> drops another
            stats = f.stats()
            self.assertGreater(stats.dropped, 0)
            unblock_sender.set()
        finally:
            f.stop(timeout=2.0)

    def test_send_errors_are_swallowed(self):
        errors = []

        def send(data: bytes) -> None:
            raise RuntimeError("boom")

        f = Forwarder(send, on_error=errors.append)
        f.start()
        try:
            f.push(b"x")
            deadline = time.time() + 1.0
            while not errors and time.time() < deadline:
                time.sleep(0.01)
            self.assertTrue(errors)
            stats = f.stats()
            self.assertEqual(stats.errors, 1)
            self.assertEqual(stats.sent, 0)
        finally:
            f.stop()

    def test_stop_is_idempotent_and_quick(self):
        f = Forwarder(lambda data: None)
        f.start()
        f.stop()
        f.stop()  # second stop should be a no-op

    def test_empty_push_is_noop(self):
        sent = []
        f = Forwarder(sent.append)
        f.start()
        try:
            f.push(b"")
            time.sleep(0.05)
            self.assertEqual(sent, [])
        finally:
            f.stop()


if __name__ == "__main__":
    unittest.main()
