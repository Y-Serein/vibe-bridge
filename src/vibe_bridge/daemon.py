"""vibe-bridge daemon.

Owns the session mirror, the plugin Unix-socket server, and the VT100 router.
Mock mode allocates session ids locally and mirrors the active session to a
screen file. Real-HID mode bridges plugin packets to ``/dev/hidraw*``; daemon
startup only probes the hidraw node, while every plugin ``REQUEST_SESSION`` is
forwarded to the board and the board-returned session id is treated as
authoritative.
"""

from __future__ import annotations

import json
import logging
import os
import errno
import threading
import time
from collections import deque
from dataclasses import dataclass
from typing import Deque, Dict, Optional, Tuple

from .hid_protocol import (
    Cmd,
    Packet,
    ProtocolError,
    Status,
    make_error,
    make_session_invalid,
    make_session_response,
    stream_iter_packets,
    SESSION_BROADCAST,
)
from .mock_hid import ClientHandle, MockHidServer, DEFAULT_SOCK_PATH
from .session_manager import SessionManager
from .transport import Transport, TransportClosed
from .vt100_router import Vt100Router

DEFAULT_STATE_PATH = "/tmp/vibe-bridge-state.json"
DEFAULT_SCREEN_PATH = "/tmp/vibe-bridge-screen.out"
DEFAULT_LOG_PATH = "/tmp/vibe-bridge-daemon.log"

log = logging.getLogger("vibe_bridge.daemon")


@dataclass
class DaemonConfig:
    sock_path: str = DEFAULT_SOCK_PATH
    state_path: str = DEFAULT_STATE_PATH
    screen_path: str = DEFAULT_SCREEN_PATH
    max_sessions: int = 256
    ttl_seconds: int = 30 * 24 * 60 * 60
    reap_interval_seconds: float = 60.0
    hidraw_path: Optional[str] = None
    hid_transport: Optional[Transport] = None


class Daemon:
    def __init__(self, config: Optional[DaemonConfig] = None) -> None:
        self.config = config or DaemonConfig()
        self.sessions = SessionManager(
            max_sessions=self.config.max_sessions,
            ttl_seconds=self.config.ttl_seconds,
        )
        self.router = Vt100Router(screen_sink=self._on_screen_write)
        self.server = MockHidServer(self._handle_packet, sock_path=self.config.sock_path)

        # Map sid -> ClientHandle for invalidation notifications and replies.
        self._owners: Dict[int, ClientHandle] = {}
        self._owners_lock = threading.Lock()
        # Serializes state-file writes so concurrent handlers don't race on the
        # ``.tmp -> final`` rename.
        self._state_lock = threading.Lock()
        self._hid: Optional[Transport] = None
        self._hid_lock = threading.Lock()
        self._hid_reader: Optional[threading.Thread] = None
        self._pending_sessions: Deque[Tuple[ClientHandle, str]] = deque()
        self._pending_lock = threading.Lock()

        self.sessions.set_invalidation_callback(self._on_session_invalidate)

        self._stop = threading.Event()
        self._reaper: Optional[threading.Thread] = None

    # ----------------------------------------------------------- lifecycle

    def start(self) -> None:
        self._truncate(self.config.screen_path)
        if self.config.hidraw_path or self.config.hid_transport is not None:
            self._start_hidraw_bridge()
        self.server.start()
        self._reaper = threading.Thread(
            target=self._reap_loop, name="vibe-bridge-reaper", daemon=True
        )
        self._reaper.start()
        self._dump_state()
        log.info("daemon listening on %s", self.config.sock_path)

    def stop(self) -> None:
        self._stop.set()
        self.server.stop()
        if self._hid is not None:
            try:
                self._hid.close()
            except Exception:
                pass
        if self._reaper is not None:
            self._reaper.join(timeout=1.0)
        if self._hid_reader is not None:
            self._hid_reader.join(timeout=1.0)

    def run_forever(self) -> None:
        self.start()
        try:
            while not self._stop.wait(timeout=1.0):
                pass
        except KeyboardInterrupt:
            pass
        finally:
            self.stop()

    def _start_hidraw_bridge(self) -> None:
        if self.config.hid_transport is not None:
            self._hid = self.config.hid_transport
            path = self.config.hidraw_path or "<injected>"
        else:
            from .transport_hidraw import HidrawTransport

            if not self.config.hidraw_path:
                raise ValueError("hidraw_path is required when hid_transport is not injected")
            self._hid = HidrawTransport(self.config.hidraw_path)
            path = self.config.hidraw_path

        self.router.set_screen_sink(self._on_hid_screen_write)
        drained = self._drain_hid_startup_input()
        self._hid_reader = threading.Thread(
            target=self._hid_reader_loop, name="vibe-bridge-hidraw", daemon=True
        )
        self._hid_reader.start()
        log.info("hidraw bridge enabled on %s (drained %d startup packet(s))", path, drained)

    def _drain_hid_startup_input(self) -> int:
        """Best-effort startup probe; intentionally does not request a session."""
        hid = self._hid
        if hid is None:
            return 0
        drained = 0
        while drained < 16:
            try:
                pkt = hid.recv_packet(timeout=0.05 if drained == 0 else 0.0)
            except (OSError, ProtocolError, TransportClosed) as exc:
                log.info("hidraw startup probe stopped: %s", exc)
                break
            if pkt is None:
                break
            drained += 1
            log.debug(
                "hidraw startup drain pkt report=0x%02x cmd=0x%02x sid=%d payload=%d",
                pkt.report_id,
                pkt.command,
                pkt.session_id,
                len(pkt.payload),
            )
        return drained

    # -------------------------------------------------------------- handler

    def _handle_packet(self, packet: Packet, client: ClientHandle) -> None:
        try:
            cmd = Cmd(packet.command)
        except ValueError:
            log.warning("unknown cmd 0x%02x from sid %d", packet.command, packet.session_id)
            client.send(make_error(packet.session_id, f"unknown cmd 0x{packet.command:02x}"))
            return

        if self._hid is not None:
            self._handle_plugin_packet_real(packet, client, cmd)
            return

        if cmd == Cmd.REQUEST_SESSION:
            self._handle_request_session(packet, client)
        elif cmd == Cmd.VT100_STREAM:
            self._handle_vt100(packet, client)
        elif cmd == Cmd.STATUS_UPDATE:
            self._handle_status_update(packet, client)
        elif cmd == Cmd.WINDOW_ACTIVATE:
            self._handle_window_activate(packet, client)
        elif cmd == Cmd.WINDOW_SWITCH:
            self._handle_window_switch(packet, client)
        else:
            log.info("ignoring cmd %s sid %d (%d bytes)", cmd.name, packet.session_id, len(packet.payload))

    def _handle_plugin_packet_real(
        self, packet: Packet, client: ClientHandle, cmd: Cmd
    ) -> None:
        if cmd == Cmd.REQUEST_SESSION:
            self._forward_session_request_to_board(packet, client)
            return

        if cmd == Cmd.VT100_STREAM:
            self._handle_vt100(packet, client)
            return

        if cmd == Cmd.STATUS_UPDATE:
            self._handle_status_update(packet, client)
            if self.sessions.get(packet.session_id) is not None:
                self._forward_packet_to_board(packet)
            return

        if cmd == Cmd.WINDOW_ACTIVATE:
            self._handle_window_activate(packet, client)
            return

        if cmd == Cmd.WINDOW_SWITCH:
            self._handle_window_switch(packet, client)
            return

        if packet.session_id != SESSION_BROADCAST and not self._validate_session(packet, client):
            return
        self._forward_packet_to_board(packet)

    def _forward_session_request_to_board(self, packet: Packet, client: ClientHandle) -> None:
        plugin_hint = packet.payload.decode("utf-8", errors="replace") or "unknown"
        try:
            with self._pending_lock:
                self._pending_sessions.append((client, plugin_hint))
                self._forward_packet_to_board(packet)
        except Exception as exc:
            self._drop_pending_session(client, plugin_hint)
            log.warning("failed to forward session request for %s: %s", plugin_hint, exc)
            try:
                client.send(make_session_invalid(SESSION_BROADCAST, Status.INVALID))
            except Exception:
                pass
            return
        log.info("forwarded session request for %s to board", plugin_hint)

    # ---------------------------------------------------- specific handlers

    def _handle_request_session(self, packet: Packet, client: ClientHandle) -> None:
        plugin_hint = packet.payload.decode("utf-8", errors="replace") or "unknown"
        sid, status, evicted = self.sessions.request_session(plugin_hint)
        if status == Status.POOL_FULL:
            client.send(make_session_invalid(SESSION_BROADCAST, Status.POOL_FULL))
            log.warning("session pool full, rejecting request from %s", plugin_hint)
            return

        with self._owners_lock:
            for victim in evicted:
                self._owners.pop(victim, None)
            self._owners[sid] = client
        self.router.register(sid)
        client.send(make_session_response(sid, status))
        log.info("granted sid=%d to %s (status=%s)", sid, plugin_hint, status.name)
        self._dump_state()

    def _handle_vt100(self, packet: Packet, client: ClientHandle) -> None:
        if not self._validate_session(packet, client):
            return
        self.sessions.touch(packet.session_id)
        self.router.append(packet.session_id, packet.payload)

    def _handle_status_update(self, packet: Packet, client: ClientHandle) -> None:
        if not self._validate_session(packet, client):
            return
        sess = self.sessions.get(packet.session_id)
        if sess is None:
            return
        try:
            update = json.loads(packet.payload.decode("utf-8")) if packet.payload else {}
        except (UnicodeDecodeError, json.JSONDecodeError):
            update = {"raw": packet.payload.hex()}
        sess.context.update(update if isinstance(update, dict) else {"value": update})
        self.sessions.touch(packet.session_id)
        self._dump_state()

    def _handle_window_activate(self, packet: Packet, client: ClientHandle) -> None:
        if not self._validate_session(packet, client):
            return
        if self.router.set_active(packet.session_id):
            log.info("active window -> sid %d", packet.session_id)
            self._dump_state()

    def _handle_window_switch(self, packet: Packet, client: ClientHandle) -> None:
        # Convention: payload[0] is a signed delta (-1 prev, +1 next, 0 = none).
        delta = 0
        if packet.payload:
            delta = packet.payload[0]
            if delta > 127:
                delta -= 256
        new_sid = self._switch_window_by_delta(delta)
        if new_sid is not None:
            log.info("window switch delta=%d -> sid %d", delta, new_sid)
            self._dump_state()

    def _switch_window_by_delta(self, delta: int) -> Optional[int]:
        sids = [s.sid for s in self.sessions.all_sessions()]
        if not sids:
            return None
        active = self.router.active()
        idx = sids.index(active) if active in sids else 0
        new_idx = (idx + delta) % len(sids)
        self.router.set_active(sids[new_idx])
        return sids[new_idx]

    # --------------------------------------------------------- bookkeeping

    def _hid_reader_loop(self) -> None:
        while not self._stop.is_set():
            hid = self._hid
            if hid is None:
                return
            try:
                pkt = hid.recv_packet(timeout=0.5)
            except TransportClosed as exc:
                log.warning("hidraw closed: %s", exc)
                return
            except OSError as exc:
                log.warning("hidraw read error: %s", exc)
                if exc.errno in (errno.EIO, errno.ENODEV):
                    log.warning("hidraw reader stopped; restart daemon after board reconnect")
                    return
                time.sleep(0.1)
                continue
            except ProtocolError as exc:
                log.warning("hidraw read error: %s", exc)
                continue
            if pkt is None:
                continue
            self._handle_hid_packet(pkt)

    def _handle_hid_packet(self, packet: Packet) -> None:
        try:
            cmd = Cmd(packet.command)
        except ValueError:
            log.info("unknown board cmd 0x%02x sid %d", packet.command, packet.session_id)
            return

        if cmd == Cmd.SESSION_RESPONSE:
            self._handle_hid_session_response(packet)
        elif cmd == Cmd.SESSION_INVALID:
            self._handle_hid_session_invalid(packet)
        elif cmd == Cmd.KEY_EVENT:
            self._route_board_input_to_active(packet)
        elif cmd == Cmd.ENCODER_EVENT:
            self._handle_hid_encoder_event(packet)
        else:
            self._route_board_packet(packet)

    def _handle_hid_session_response(self, packet: Packet) -> None:
        pending = self._pop_pending_session()
        if pending is None:
            log.info(
                "dropping unclaimed SESSION_RESPONSE sid=%d payload=%r",
                packet.session_id,
                packet.payload,
            )
            return
        client, plugin_hint = pending
        status = self._status_from_payload(packet.payload, Status.OK)

        if status in (Status.OK, Status.CREATED) and packet.session_id != SESSION_BROADCAST:
            sid, local_status, _ = self.sessions.adopt_session(packet.session_id, plugin_hint)
            if local_status in (Status.CREATED, Status.OK) and sid == packet.session_id:
                with self._owners_lock:
                    self._owners[sid] = client
                self.router.register(sid)
                log.info("board granted sid=%d to %s (status=%s)", sid, plugin_hint, status.name)
                self._dump_state()
            else:
                log.warning(
                    "board granted unusable sid=%d to %s (local_status=%s)",
                    packet.session_id,
                    plugin_hint,
                    local_status.name,
                )
                try:
                    client.send(make_session_invalid(packet.session_id, local_status))
                except Exception:
                    pass
                return
        else:
            log.info("board rejected session for %s (status=%s)", plugin_hint, status.name)
            reject_status = status if status not in (Status.OK, Status.CREATED) else Status.INVALID
            try:
                client.send(make_session_invalid(packet.session_id, reject_status))
            except Exception:
                pass
            return

        try:
            client.send(packet)
        except Exception:
            pass

    def _handle_hid_session_invalid(self, packet: Packet) -> None:
        status = self._status_from_payload(packet.payload, Status.INVALID)
        if packet.session_id == SESSION_BROADCAST:
            pending = self._pop_pending_session()
            if pending is not None:
                client, _ = pending
                try:
                    client.send(packet)
                except Exception:
                    pass
                return
            self.server.broadcast(packet)
            return
        self.sessions.invalidate(packet.session_id, status)

    def _handle_hid_encoder_event(self, packet: Packet) -> None:
        delta = 0
        if packet.payload:
            delta = packet.payload[0]
            if delta > 127:
                delta -= 256
        new_sid = self._switch_window_by_delta(delta)
        if new_sid is not None:
            log.info("board encoder delta=%d -> sid %d", delta, new_sid)
            self._dump_state()
        self._route_board_input_to_active(packet)

    def _route_board_input_to_active(self, packet: Packet) -> None:
        active = self.router.active()
        if active is None:
            return
        with self._owners_lock:
            owner = self._owners.get(active)
        if owner is None:
            return
        routed = Packet(
            report_id=packet.report_id,
            command=packet.command,
            session_id=active,
            payload=packet.payload,
        )
        try:
            owner.send(routed)
        except Exception:
            pass

    def _route_board_packet(self, packet: Packet) -> None:
        if packet.session_id == SESSION_BROADCAST:
            self.server.broadcast(packet)
            return
        with self._owners_lock:
            owner = self._owners.get(packet.session_id)
        if owner is None:
            log.info("dropping board packet cmd=%s sid=%d with no owner", packet.command, packet.session_id)
            return
        try:
            owner.send(packet)
        except Exception:
            pass

    def _on_hid_screen_write(self, sid: int, data: bytes) -> None:
        for pkt in stream_iter_packets(sid, data):
            self._forward_packet_to_board(pkt)

    def _forward_packet_to_board(self, packet: Packet) -> None:
        hid = self._hid
        if hid is None:
            raise TransportClosed("hidraw bridge is not enabled")
        with self._hid_lock:
            hid.send_packet(packet)

    def _pop_pending_session(self) -> Optional[Tuple[ClientHandle, str]]:
        with self._pending_lock:
            if not self._pending_sessions:
                return None
            return self._pending_sessions.popleft()

    def _drop_pending_session(self, client: ClientHandle, plugin_hint: str) -> None:
        with self._pending_lock:
            try:
                self._pending_sessions.remove((client, plugin_hint))
            except ValueError:
                pass

    @staticmethod
    def _status_from_payload(payload: bytes, default: Status) -> Status:
        if not payload:
            return default
        try:
            return Status(payload[0])
        except ValueError:
            return default

    def _validate_session(self, packet: Packet, client: ClientHandle) -> bool:
        if self.sessions.get(packet.session_id) is None:
            log.info(
                "rejecting cmd 0x%02x for unknown sid %d", packet.command, packet.session_id
            )
            try:
                client.send(make_session_invalid(packet.session_id, Status.INVALID))
            except Exception:
                pass
            return False
        # Rebind the owner: a wrapper may have exec'd into the real CLI (closing
        # its socket) and a follower process — sharing VIBE_SESSION_ID — has now
        # connected. Future SESSION_INVALID notifications go to the latest live
        # client.
        with self._owners_lock:
            if self._owners.get(packet.session_id) is not client:
                self._owners[packet.session_id] = client
        return True

    def _on_session_invalidate(self, sid: int, status: Status) -> None:
        self.router.unregister(sid)
        with self._owners_lock:
            owner = self._owners.pop(sid, None)
        if owner is not None:
            try:
                owner.send(make_session_invalid(sid, status))
            except Exception:
                pass
        log.info("session %d invalidated (%s)", sid, status.name)
        self._dump_state()

    def _on_screen_write(self, sid: int, data: bytes) -> None:
        path = self.config.screen_path
        try:
            with open(path, "ab") as f:
                f.write(data)
        except OSError as exc:
            log.warning("failed to write screen %s: %s", path, exc)

    def _reap_loop(self) -> None:
        while not self._stop.wait(timeout=self.config.reap_interval_seconds):
            self.sessions.reap_expired()

    def _dump_state(self) -> None:
        state = {
            "sock_path": self.config.sock_path,
            "mode": "real-hidraw" if self._hid is not None else "mock",
            "hidraw_path": self.config.hidraw_path,
            "active_sid": self.router.active(),
            "sessions": [
                {
                    "sid": s.sid,
                    "plugin": s.plugin,
                    "cwd": s.cwd,
                    "created_ms": s.created_ms,
                    "last_active_ms": s.last_active_ms,
                    "context": s.context,
                }
                for s in self.sessions.all_sessions()
            ],
            "buffers": dict(self.router.buffer_sizes()),
            "ts_ms": int(time.time() * 1000),
        }
        path = self.config.state_path
        tmp = path + ".tmp"
        with self._state_lock:
            try:
                with open(tmp, "w", encoding="utf-8") as f:
                    json.dump(state, f, indent=2, sort_keys=True)
                os.replace(tmp, path)
            except OSError as exc:
                log.warning("failed to write state %s: %s", path, exc)

    @staticmethod
    def _truncate(path: str) -> None:
        try:
            with open(path, "wb"):
                pass
        except OSError:
            pass
