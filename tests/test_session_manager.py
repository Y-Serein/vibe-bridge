import unittest

from vibe_bridge.hid_protocol import Status
from vibe_bridge.session_manager import SessionManager


class FakeClock:
    def __init__(self, start: int = 0) -> None:
        self.now = start

    def __call__(self) -> int:
        return self.now

    def advance(self, ms: int) -> None:
        self.now += ms


class SessionManagerTests(unittest.TestCase):
    def test_first_session_is_one(self):
        sm = SessionManager()
        sid, status, evicted = sm.request_session("a")
        self.assertEqual(sid, 1)
        self.assertIs(status, Status.CREATED)
        self.assertEqual(evicted, [])

    def test_distinct_sessions_get_distinct_ids(self):
        sm = SessionManager()
        sid_a, _, _ = sm.request_session("a")
        sid_b, _, _ = sm.request_session("b")
        self.assertNotEqual(sid_a, sid_b)
        self.assertEqual({s.sid for s in sm.all_sessions()}, {sid_a, sid_b})

    def test_invalidate_removes_session_and_invokes_callback(self):
        notes = []
        sm = SessionManager()
        sm.set_invalidation_callback(lambda sid, status: notes.append((sid, status)))
        sid, _, _ = sm.request_session("a")
        self.assertTrue(sm.invalidate(sid))
        self.assertIsNone(sm.get(sid))
        self.assertEqual(notes, [(sid, Status.INVALID)])

    def test_lru_eviction_when_pool_full(self):
        notes = []
        clock = FakeClock(0)
        sm = SessionManager(max_sessions=2, ttl_seconds=10**9, clock_ms=clock)
        sm.set_invalidation_callback(lambda sid, status: notes.append((sid, status)))

        sid_a, _, _ = sm.request_session("a")
        clock.advance(10)
        sid_b, _, _ = sm.request_session("b")
        clock.advance(10)
        sm.touch(sid_a)  # B is now LRU
        clock.advance(10)
        sid_c, status, evicted = sm.request_session("c")
        self.assertIs(status, Status.CREATED)
        self.assertIn(sid_b, evicted)
        self.assertIn((sid_b, Status.RECLAIMED), notes)
        # B's owner has been notified; the session table now holds A and C even
        # though C may have recycled B's numeric sid.
        plugins = {s.plugin for s in sm.all_sessions()}
        self.assertEqual(plugins, {"a", "c"})
        self.assertEqual(len(sm), 2)

    def test_ttl_expiry(self):
        notes = []
        clock = FakeClock(0)
        sm = SessionManager(max_sessions=4, ttl_seconds=1, clock_ms=clock)
        sm.set_invalidation_callback(lambda sid, status: notes.append((sid, status)))
        sid, _, _ = sm.request_session("a")
        clock.advance(2000)
        expired = sm.reap_expired()
        self.assertEqual(expired, [sid])
        self.assertIn((sid, Status.EXPIRED), notes)
        self.assertIsNone(sm.get(sid))

    def test_request_after_eviction_recycles_sid_space(self):
        sm = SessionManager(max_sessions=2)
        sid_a, _, _ = sm.request_session("a")
        sid_b, _, _ = sm.request_session("b")
        sm.invalidate(sid_a)
        sid_c, status, _ = sm.request_session("c")
        self.assertIs(status, Status.CREATED)
        self.assertNotEqual(sid_c, sid_b)

    def test_touch_updates_last_active(self):
        clock = FakeClock(100)
        sm = SessionManager(clock_ms=clock)
        sid, _, _ = sm.request_session("a")
        initial = sm.get(sid).last_active_ms
        clock.advance(50)
        self.assertTrue(sm.touch(sid))
        self.assertEqual(sm.get(sid).last_active_ms, initial + 50)

    def test_adopt_session_registers_board_assigned_sid(self):
        sm = SessionManager(max_sessions=256)
        sid, status, evicted = sm.adopt_session(42, "board")
        self.assertEqual(sid, 42)
        self.assertIs(status, Status.CREATED)
        self.assertEqual(evicted, [])
        self.assertEqual(sm.get(42).plugin, "board")

    def test_adopt_existing_sid_reclaims_previous_owner(self):
        notes = []
        sm = SessionManager(max_sessions=256)
        sm.set_invalidation_callback(lambda sid, status: notes.append((sid, status)))
        sm.adopt_session(7, "old")
        sid, status, evicted = sm.adopt_session(7, "new")
        self.assertEqual((sid, status, evicted), (7, Status.CREATED, [7]))
        self.assertEqual(sm.get(7).plugin, "new")
        self.assertIn((7, Status.RECLAIMED), notes)


if __name__ == "__main__":
    unittest.main()
