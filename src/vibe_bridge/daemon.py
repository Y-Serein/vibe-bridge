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
import queue
from collections import deque
from dataclasses import dataclass
from typing import Deque, Dict, Optional, Tuple

from .hid_protocol import (
    Cmd,
    Packet,
    ProtocolError,
    Status,
    decode_encoder_delta_payload,
    decode_key_event_payload,
    make_error,
    make_session_heartbeat,
    make_session_invalid,
    make_session_response,
    stream_iter_packets,
    SESSION_BROADCAST,
)
from .agent_scanner import AgentScanner, DEFAULT_SCAN_INTERVAL_SECONDS
from .mock_hid import ClientHandle, MockHidServer, DEFAULT_SOCK_PATH
from .session_manager import SessionManager
from .transport import Transport, TransportClosed
from .vt100_router import Vt100Router

DEFAULT_STATE_PATH = "/tmp/vibe-bridge-state.json"
DEFAULT_SCREEN_PATH = "/tmp/vibe-bridge-screen.out"
DEFAULT_LOG_PATH = "/tmp/vibe-bridge-daemon.log"
HID_TX_QUEUE_LIMIT = 4096
# Board-side timeout is 30 s without a heartbeat; emit one every 10 s so a
# single dropped packet doesn't flip a session to DISCONNECTED.
SESSION_HEARTBEAT_INTERVAL_SECONDS = 10.0

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
    hid_mode: str = "real-hidraw"
    heartbeat_interval_seconds: float = SESSION_HEARTBEAT_INTERVAL_SECONDS
    agent_scan_enabled: bool = True
    agent_scan_interval_seconds: float = DEFAULT_SCAN_INTERVAL_SECONDS


class Daemon:
    def __init__(self, config: Optional[DaemonConfig] = None) -> None:
        self.config = config or DaemonConfig()
        self.sessions = SessionManager(
            max_sessions=self.config.max_sessions,
            ttl_seconds=self.config.ttl_seconds,
        )
        self.router = Vt100Router(screen_sink=self._on_screen_write)
        self.server = MockHidServer(
            self._handle_packet,
            sock_path=self.config.sock_path,
            on_disconnect=self._handle_client_disconnect,
        )

        # Map sid -> ClientHandle for invalidation notifications and replies.
        self._owners: Dict[int, ClientHandle] = {}
        self._owners_lock = threading.Lock()
        # Serializes state-file writes so concurrent handlers don't race on the
        # ``.tmp -> final`` rename.
        self._state_lock = threading.Lock()
        self._hid: Optional[Transport] = None
        self._hid_lock = threading.Lock()
        self._hid_reader: Optional[threading.Thread] = None
        self._hid_writer: Optional[threading.Thread] = None
        self._hid_tx_queue: "queue.Queue[Optional[Packet]]" = queue.Queue(
            maxsize=HID_TX_QUEUE_LIMIT
        )
        self._pending_sessions: Deque[Tuple[ClientHandle, str]] = deque()
        self._pending_lock = threading.Lock()
        # Board owns the picker / terminal view; host only learns which sid the
        # user confirmed via CMD_SESSION_FOCUS. The VT100 stream gate uses this
        # value (None = nothing focused → no bytes forwarded to the board).
        self._focused_sid: Optional[int] = None
        self._last_state_dump_ms = 0
        self._last_board_input_monotonic = time.monotonic()
        self._last_board_event = ""
        self._last_board_event_seq = 0
        self._last_board_event_ms = 0
        self._last_board_tx = ""

        self.sessions.set_invalidation_callback(self._on_session_invalidate)

        self._stop = threading.Event()
        self._reaper: Optional[threading.Thread] = None
        self._heartbeat: Optional[threading.Thread] = None
        self._agent_scanner: Optional[AgentScanner] = None

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
        self._heartbeat = threading.Thread(
            target=self._heartbeat_loop,
            name="vibe-bridge-heartbeat",
            daemon=True,
        )
        self._heartbeat.start()
        if self.config.agent_scan_enabled:
            self._agent_scanner = AgentScanner(
                sock_path=self.config.sock_path,
                interval_seconds=self.config.agent_scan_interval_seconds,
            )
            self._agent_scanner.start()
        self._dump_state()
        log.info("daemon listening on %s", self.config.sock_path)

    def stop(self) -> None:
        self._stop.set()
        if self._agent_scanner is not None:
            self._agent_scanner.stop()
        self.server.stop()
        if self._hid is not None:
            try:
                self._hid.close()
            except Exception:
                pass
        try:
            self._hid_tx_queue.put_nowait(None)
        except queue.Full:
            pass
        if self._reaper is not None:
            self._reaper.join(timeout=1.0)
        if self._heartbeat is not None:
            self._heartbeat.join(timeout=1.0)
        if self._hid_reader is not None:
            self._hid_reader.join(timeout=1.0)
        if self._hid_writer is not None:
            self._hid_writer.join(timeout=1.0)

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
        self._hid_writer = threading.Thread(
            target=self._hid_writer_loop, name="vibe-bridge-hidraw-writer", daemon=True
        )
        self._hid_writer.start()
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

        if cmd in (Cmd.WINDOW_ACTIVATE, Cmd.WINDOW_SWITCH):
            # Deprecated: host can no longer drive board view transitions.
            log.info("dropping deprecated %s from plugin sid=%d", cmd.name, packet.session_id)
            return

        if packet.session_id != SESSION_BROADCAST and not self._validate_session(packet, client):
            return
        self._forward_packet_to_board(packet)

    def _forward_session_request_to_board(self, packet: Packet, client: ClientHandle) -> None:
        plugin_hint = packet.payload.decode("utf-8", errors="replace") or "unknown"
        with self._pending_lock:
            self._pending_sessions.append((client, plugin_hint))
        try:
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
        self._dump_state_throttled()

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
                self._mark_hidraw_unavailable()
                return
            except OSError as exc:
                log.warning("hidraw read error: %s", exc)
                if exc.errno in (errno.EIO, errno.ENODEV):
                    log.warning("hidraw reader stopped; restart daemon after board reconnect")
                    self._mark_hidraw_unavailable()
                    return
                time.sleep(0.1)
                continue
            except ProtocolError as exc:
                log.warning("hidraw read error: %s", exc)
                continue
            if pkt is None:
                continue
            self._handle_hid_packet(pkt)

    def _hid_writer_loop(self) -> None:
        while not self._stop.is_set():
            try:
                packet = self._hid_tx_queue.get(timeout=0.5)
            except queue.Empty:
                continue
            if packet is None:
                return
            with self._hid_lock:
                hid = self._hid
            if hid is None:
                continue
            try:
                hid.send_packet(packet)
            except TransportClosed as exc:
                log.warning("hidraw write closed: %s", exc)
                self._mark_hidraw_unavailable()
                return
            except OSError as exc:
                log.warning("hidraw write error: %s", exc)
                self._mark_hidraw_unavailable()
                return
            except ProtocolError as exc:
                log.warning("hidraw write error: %s", exc)
                continue

    def _mark_hidraw_unavailable(self) -> None:
        with self._hid_lock:
            hid = self._hid
            self._hid = None
            self.config.hidraw_path = None
        if hid is not None:
            try:
                hid.close()
            except Exception:
                pass
        try:
            self._hid_tx_queue.put_nowait(None)
        except queue.Full:
            pass
        self._fail_pending_sessions(Status.INVALID)
        self._dump_state()

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
        elif cmd == Cmd.SESSION_FOCUS:
            self._handle_hid_session_focus(packet)
        elif cmd == Cmd.KEY_EVENT:
            self._handle_hid_key_event(packet)
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
                old_sids = self._sessions_owned_by(client, except_sid=sid)
                with self._owners_lock:
                    for old_sid in old_sids:
                        self._owners.pop(old_sid, None)
                    self._owners[sid] = client
                self.router.register(sid)
                for old_sid in old_sids:
                    self.sessions.invalidate(old_sid, Status.RECLAIMED)
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

    def _handle_hid_session_focus(self, packet: Packet) -> None:
        """Board user picked ``packet.session_id``. Open the VT100 gate so the
        active session's bytes start flowing toward the LCD."""
        sid = packet.session_id
        if sid == SESSION_BROADCAST:
            # Treat sid=0 as "unfocus" — board left the terminal view.
            self._focused_sid = None
            self.router.set_active(None)
            log.info("board cleared focus")
            self._dump_state()
            return
        if self.sessions.get(sid) is None:
            log.info("board focused unknown sid %d; ignoring", sid)
            return
        self._focused_sid = sid
        self.sessions.touch(sid)
        # router.set_active flushes a screen-clear + the per-sid replay so the
        # board's terminal lights up with the latest state for this window.
        self.router.set_active(sid)
        log.info("board focused sid=%d", sid)
        self._dump_state()

    def _handle_hid_key_event(self, packet: Packet) -> None:
        """Board → host key event. Per the new contract the board owns its own
        UI, so the daemon's only job is to forward the event to the plugin that
        owns the (board-stamped) sid."""
        self._mark_board_interaction()
        try:
            event = decode_key_event_payload(packet.payload)
        except ProtocolError as exc:
            log.info("dropping invalid KEY_EVENT: %s", exc)
            self._record_board_event(f"invalid-key:{exc}")
            return
        self._record_board_event(
            f"key bits=0x{event.key_bits:02x} enc={1 if event.encoder_pressed else 0} sid={packet.session_id}"
        )
        self._route_board_input_to_session(packet)

    def _handle_hid_encoder_event(self, packet: Packet) -> None:
        self._mark_board_interaction()
        try:
            delta = decode_encoder_delta_payload(packet.payload)
        except ProtocolError as exc:
            log.info("dropping invalid ENCODER_EVENT: %s", exc)
            self._record_board_event(f"invalid-encoder:{exc}")
            return
        self._record_board_event(f"encoder delta={delta} sid={packet.session_id}")
        self._route_board_input_to_session(packet)

    def _route_board_input_to_session(self, packet: Packet) -> None:
        """Deliver a board-stamped KEY/ENCODER event to the plugin owning that sid.

        ``aikb_hid_input`` only emits these packets while the board is in the
        terminal view, so the daemon trusts the firmware-stamped sid and does
        not try to second-guess it.
        """
        sid = packet.session_id
        if sid == SESSION_BROADCAST:
            log.info("dropping board %s with sid=0", Cmd(packet.command).name)
            return
        with self._owners_lock:
            owner = self._owners.get(sid)
        if owner is None:
            log.info("dropping board %s sid=%d (no owner)", Cmd(packet.command).name, sid)
            return
        try:
            owner.send(packet)
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
        """VT100 sink invoked by ``Vt100Router`` whenever the active sid's
        buffer grows. The router only triggers for the focused sid, which is
        exactly what we want — host stops streaming for any window the board
        has not selected."""
        if self._focused_sid is None or sid != self._focused_sid:
            return
        if self._hid is None:
            return
        try:
            for pkt in stream_iter_packets(sid, data):
                self._forward_packet_to_board(pkt)
        except Exception as exc:
            log.info("hidraw screen write dropped: %s", exc)

    def _forward_packet_to_board(self, packet: Packet) -> None:
        with self._hid_lock:
            hid = self._hid
        if hid is None:
            raise TransportClosed("hidraw bridge is not enabled")
        self._last_board_tx = (
            f"cmd=0x{int(packet.command):02x} sid={packet.session_id} len={len(packet.payload)}"
        )
        self._dump_state_throttled()
        try:
            self._hid_tx_queue.put_nowait(packet)
        except queue.Full as exc:
            log.warning("hidraw tx queue full; marking device unavailable")
            self._mark_hidraw_unavailable()
            raise TransportClosed("hidraw tx queue full") from exc

    def _mark_board_interaction(self) -> None:
        self._last_board_input_monotonic = time.monotonic()

    def _record_board_event(self, event: str) -> None:
        self._last_board_event = event
        self._last_board_event_seq += 1
        self._last_board_event_ms = int(time.time() * 1000)
        self._dump_state_throttled()

    def _sessions_owned_by(
        self, client: ClientHandle, *, except_sid: Optional[int] = None
    ) -> list[int]:
        with self._owners_lock:
            return [
                sid for sid, owner in self._owners.items()
                if owner is client and sid != except_sid
            ]

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

    def _fail_pending_sessions(self, status: Status) -> None:
        with self._pending_lock:
            pending = list(self._pending_sessions)
            self._pending_sessions.clear()
        for client, _ in pending:
            try:
                client.send(make_session_invalid(SESSION_BROADCAST, status))
            except Exception:
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
        if self._focused_sid == sid:
            self._focused_sid = None
        log.info("session %d invalidated (%s)", sid, status.name)
        self._dump_state()

    def _handle_client_disconnect(self, client: ClientHandle) -> None:
        released = []
        with self._owners_lock:
            for sid, owner in list(self._owners.items()):
                if owner is client:
                    released.append(sid)
        for sid in released:
            self.sessions.invalidate(sid, Status.EXPIRED)
        if released:
            log.info("client disconnected, released session(s): %s", released)

    def _on_screen_write(self, sid: int, data: bytes) -> None:
        path = self.config.screen_path
        try:
            with open(path, "ab") as f:
                f.write(data)
        except OSError as exc:
            log.warning("failed to write screen %s: %s", path, exc)

    def _reap_loop(self) -> None:
        while not self._stop.wait(timeout=self.config.reap_interval_seconds):
            expired = self.sessions.reap_expired()
            if expired:
                log.info("reaped expired sessions: %s", expired)
                self._dump_state()

    def _heartbeat_loop(self) -> None:
        """Emit CMD_SESSION_HEARTBEAT every ``heartbeat_interval_seconds`` for
        every live session. Without this, the board's 30 s reaper flips every
        session to DISCONNECTED and the VT100 stream gate slams shut."""
        while True:
            interval = max(0.01, float(self.config.heartbeat_interval_seconds))
            if self._stop.wait(timeout=interval):
                return
            if self._hid is None:
                continue
            for sess in self.sessions.all_sessions():
                pkt = make_session_heartbeat(sess.sid)
                try:
                    self._forward_packet_to_board(pkt)
                except TransportClosed:
                    # hidraw went away; the writer loop will mark it.
                    break
                except Exception as exc:
                    log.info("heartbeat for sid=%d failed: %s", sess.sid, exc)

    def _dump_state(self) -> None:
        hooked_agents = (
            self._agent_scanner.hook_table() if self._agent_scanner is not None else []
        )
        state = {
            "sock_path": self.config.sock_path,
            "mode": self.config.hid_mode if self._hid is not None else "mock",
            "hidraw_path": self.config.hidraw_path,
            "focused_sid": self._focused_sid,
            "hooked_agents": hooked_agents,
            "last_board_event": self._last_board_event,
            "last_board_event_seq": self._last_board_event_seq,
            "last_board_event_ms": self._last_board_event_ms,
            "last_board_tx": self._last_board_tx,
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

    def _dump_state_throttled(self, *, min_interval_ms: int = 250) -> None:
        now = int(time.time() * 1000)
        if now - self._last_state_dump_ms < min_interval_ms:
            return
        self._last_state_dump_ms = now
        self._dump_state()

    @staticmethod
    def _truncate(path: str) -> None:
        try:
            with open(path, "wb"):
                pass
        except OSError:
            pass
