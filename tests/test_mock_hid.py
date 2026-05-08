import os
import tempfile
import threading
import time
import unittest
from typing import List, Tuple

from vibe_bridge.hid_protocol import (
    Cmd,
    Packet,
    Status,
    make_request_session,
    make_session_response,
)
from vibe_bridge.mock_hid import ClientHandle, MockHidClient, MockHidServer


def _tmp_sock() -> str:
    fd, path = tempfile.mkstemp(prefix="vibe-bridge-test-", suffix=".sock")
    os.close(fd)
    os.unlink(path)
    return path


class MockHidTests(unittest.TestCase):
    def test_client_request_gets_server_response(self):
        sock_path = _tmp_sock()
        received: List[Tuple[Packet, ClientHandle]] = []

        def handler(pkt: Packet, client: ClientHandle) -> None:
            received.append((pkt, client))
            if pkt.command == int(Cmd.REQUEST_SESSION):
                client.send(make_session_response(123, Status.CREATED))

        server = MockHidServer(handler, sock_path=sock_path)
        server.start()
        try:
            client = MockHidClient(sock_path)
            try:
                client.send_packet(make_request_session(hint=b"unit"))
                reply = client.recv_packet(timeout=1.0)
                self.assertIsNotNone(reply)
                self.assertEqual(reply.command, int(Cmd.SESSION_RESPONSE))
                self.assertEqual(reply.session_id, 123)
                self.assertEqual(reply.payload, bytes([int(Status.CREATED)]))
            finally:
                client.close()

            deadline = time.time() + 1.0
            while not received and time.time() < deadline:
                time.sleep(0.01)
            self.assertTrue(received)
            first_pkt, _ = received[0]
            self.assertEqual(first_pkt.command, int(Cmd.REQUEST_SESSION))
            self.assertEqual(first_pkt.payload, b"unit")
        finally:
            server.stop()

    def test_two_clients_each_get_their_own_response(self):
        sock_path = _tmp_sock()
        counter = {"n": 0}
        lock = threading.Lock()

        def handler(pkt: Packet, client: ClientHandle) -> None:
            with lock:
                counter["n"] += 1
                sid = counter["n"]
            client.send(make_session_response(sid, Status.CREATED))

        server = MockHidServer(handler, sock_path=sock_path)
        server.start()
        try:
            c1 = MockHidClient(sock_path)
            c2 = MockHidClient(sock_path)
            try:
                c1.send_packet(make_request_session(hint=b"a"))
                c2.send_packet(make_request_session(hint=b"b"))
                r1 = c1.recv_packet(timeout=1.0)
                r2 = c2.recv_packet(timeout=1.0)
                self.assertIsNotNone(r1)
                self.assertIsNotNone(r2)
                self.assertNotEqual(r1.session_id, r2.session_id)
                self.assertEqual({r1.session_id, r2.session_id}, {1, 2})
            finally:
                c1.close()
                c2.close()
        finally:
            server.stop()

    def test_recv_timeout_returns_none(self):
        sock_path = _tmp_sock()
        server = MockHidServer(lambda pkt, c: None, sock_path=sock_path)
        server.start()
        try:
            c = MockHidClient(sock_path)
            try:
                self.assertIsNone(c.recv_packet(timeout=0.1))
            finally:
                c.close()
        finally:
            server.stop()


if __name__ == "__main__":
    unittest.main()
