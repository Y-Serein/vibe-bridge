import unittest

from vibe_bridge.vt100_router import SCREEN_CLEAR, Vt100Router


class Vt100RouterTests(unittest.TestCase):
    def setUp(self):
        self.calls = []
        self.router = Vt100Router(screen_sink=lambda sid, data: self.calls.append((sid, data)))

    def test_buffers_are_isolated_per_sid(self):
        self.router.register(1)
        self.router.register(2)
        self.router.append(1, b"AAA")
        self.router.append(2, b"BBB")
        self.assertEqual(self.router.snapshot(1), b"AAA")
        self.assertEqual(self.router.snapshot(2), b"BBB")

    def test_only_active_session_writes_to_sink_on_append(self):
        self.router.register(1)
        self.router.register(2)
        # sid 1 is active because it registered first.
        self.router.append(1, b"hello")
        self.router.append(2, b"hidden")
        self.assertEqual(self.calls, [(1, b"hello")])

    def test_set_active_replays_buffer_with_clear(self):
        self.router.register(1)
        self.router.register(2)
        self.router.append(1, b"AAA")
        self.router.append(2, b"BBB")
        self.calls.clear()

        self.assertTrue(self.router.set_active(2))
        self.assertEqual(self.calls, [(2, SCREEN_CLEAR + b"BBB")])

    def test_set_active_same_sid_is_noop(self):
        self.router.register(1)
        self.router.append(1, b"x")
        self.calls.clear()
        self.assertTrue(self.router.set_active(1))
        self.assertEqual(self.calls, [])

    def test_set_active_unknown_returns_false(self):
        self.router.register(1)
        self.assertFalse(self.router.set_active(99))

    def test_unregister_active_replays_replacement_buffer(self):
        self.router.register(1)
        self.router.register(2)
        self.router.append(1, b"AAA")
        self.router.append(2, b"BBB")
        self.assertEqual(self.router.active(), 1)
        self.calls.clear()

        self.router.unregister(1)
        # active should fall back to sid 2 and replay its buffer.
        self.assertEqual(self.router.active(), 2)
        self.assertEqual(self.calls, [(2, SCREEN_CLEAR + b"BBB")])

    def test_unregister_active_with_no_replacement_clears_screen(self):
        self.router.register(1)
        self.router.append(1, b"AAA")
        self.calls.clear()
        self.router.unregister(1)
        self.assertIsNone(self.router.active())
        self.assertEqual(self.calls, [(0, SCREEN_CLEAR)])

    def test_set_active_to_none_clears_when_active_was_set(self):
        self.router.register(1)
        self.router.append(1, b"AAA")
        self.calls.clear()
        self.assertTrue(self.router.set_active(None))
        self.assertEqual(self.calls, [(0, SCREEN_CLEAR)])

    def test_buffer_eviction_drops_oldest(self):
        # tiny buffer to force eviction
        r = Vt100Router(buffer_bytes=4)
        r.register(1)
        r.append(1, b"AAAA")
        r.append(1, b"BBBB")
        # Both chunks are 4 bytes; the first should be evicted.
        snap = r.snapshot(1)
        self.assertLessEqual(len(snap), 4)
        self.assertEqual(snap, b"BBBB")


if __name__ == "__main__":
    unittest.main()
