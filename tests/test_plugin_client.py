import unittest
from unittest import mock

from vibe_bridge.hid_protocol import (
    BoardKey,
    Cmd,
    Status,
    make_encoder_event,
    make_key_event,
    make_session_invalid,
)
from vibe_bridge.plugin_client import PluginClient


class PluginClientBoardEventTests(unittest.TestCase):
    def test_dispatch_routes_key_and_encoder_events_to_callback(self):
        received = []
        client = PluginClient(plugin_name="codex", on_board_packet=received.append)

        key = make_key_event(1 << BoardKey.VOTE_REVIEW)
        encoder = make_encoder_event(-1)
        client._dispatch(key)
        client._dispatch(encoder)

        self.assertEqual(received, [key, encoder])

    def test_dispatch_ignores_board_events_without_callback(self):
        client = PluginClient(plugin_name="codex")

        client._dispatch(make_key_event(1 << BoardKey.VOICE))

        self.assertIsNone(client.session_id)

    def test_set_board_packet_handler_replaces_callback(self):
        first = []
        second = []
        client = PluginClient(plugin_name="codex", on_board_packet=first.append)

        client.set_board_packet_handler(second.append)
        client._dispatch(make_key_event(1 << BoardKey.MENU_DEBUG))

        self.assertEqual(first, [])
        self.assertEqual(len(second), 1)
        self.assertEqual(second[0].command, int(Cmd.KEY_EVENT))

    def test_session_invalid_for_other_sid_is_ignored(self):
        client = PluginClient(plugin_name="codex")
        client.adopt_session(42)

        with mock.patch.object(client, "_client") as transport:
            client._dispatch(make_session_invalid(99, Status.RECLAIMED))

        self.assertEqual(client.session_id, 42)
        transport.send_packet.assert_not_called()

    def test_session_invalid_for_current_sid_can_reacquire(self):
        client = PluginClient(plugin_name="codex")
        client.adopt_session(42)

        with mock.patch.object(client, "_client") as transport:
            client._dispatch(make_session_invalid(42, Status.RECLAIMED))

        self.assertIsNone(client.session_id)
        transport.send_packet.assert_called_once()

    def test_session_invalid_for_current_sid_respects_no_auto_reacquire(self):
        client = PluginClient(plugin_name="codex", auto_reacquire=False)
        client.adopt_session(42)

        with mock.patch.object(client, "_client") as transport:
            client._dispatch(make_session_invalid(42, Status.RECLAIMED))

        self.assertIsNone(client.session_id)
        transport.send_packet.assert_not_called()


if __name__ == "__main__":
    unittest.main()
