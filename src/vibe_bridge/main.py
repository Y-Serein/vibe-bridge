"""vibe-bridge CLI entry point."""

from __future__ import annotations

import argparse
import json
import logging
import os
import shutil
import socket
import sys
import time
from typing import Optional

from .bootstrap import (
    VIBE_USB_PID,
    VIBE_USB_VID,
    can_connect,
    ensure_daemon_running,
    resolve_hidraw_device,
)
from .daemon import Daemon, DaemonConfig, DEFAULT_STATE_PATH
from .hid_protocol import (
    Cmd,
    Packet,
    ProtocolError,
    ReportId,
    SESSION_BROADCAST,
    Status,
    make_request_session,
    make_vt100_chunk,
    stream_iter_packets,
)
from .mock_hid import MockHidClient, DEFAULT_SOCK_PATH
from .mock_hid import connect_packet_socket, is_tcp_endpoint


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="vibe-bridge", description="Multi-window HID session bridge")
    p.add_argument("--sock", default=DEFAULT_SOCK_PATH, help="Unix socket path (default: %(default)s)")
    p.add_argument("--verbose", "-v", action="count", default=0)

    sub = p.add_subparsers(dest="cmd", required=True)

    pd = sub.add_parser("daemon", help="Run the bridge daemon in the foreground")
    pd.add_argument("--state", default=DEFAULT_STATE_PATH)
    pd.add_argument("--screen", default=None)
    pd.add_argument("--max-sessions", type=int, default=256)
    pd.add_argument(
        "--hidraw",
        default=None,
        help="Bridge to a real /dev/hidraw* device; session ids remain board-assigned",
    )
    pd.add_argument(
        "--winhid",
        default=None,
        help="Bridge to a native Windows HID device path; use 'auto' to select 359f:2120",
    )

    ped = sub.add_parser(
        "ensure-daemon",
        help="Start a detached daemon if needed, then report socket/log status",
    )
    ped.add_argument("--state", default=DEFAULT_STATE_PATH)
    ped.add_argument("--log", default="/tmp/vibe-bridge-daemon.log")
    ped.add_argument("--timeout", type=float, default=3.0)

    pdoc = sub.add_parser("doctor", help="Check host install, daemon, HID, and wrapper health")
    pdoc.add_argument("--state", default=DEFAULT_STATE_PATH)
    pdoc.add_argument("--log", default="/tmp/vibe-bridge-daemon.log")
    pdoc.add_argument("--cli", action="append", default=None,
                      help="CLI wrapper name to check; may be passed multiple times")
    pdoc.add_argument(
        "--color",
        choices=("auto", "always", "never"),
        default="auto",
        help="Colorize doctor status labels (default: %(default)s)",
    )

    ps = sub.add_parser("sessions", help="Print the daemon session table (reads state file)")
    ps.add_argument("--state", default=DEFAULT_STATE_PATH)

    pr = sub.add_parser("request-session", help="Smoke test: connect, request a session, print result")
    pr.add_argument("--plugin", default="cli-smoke")
    pr.add_argument("--cwd", default=os.getcwd())

    pv = sub.add_parser("send-vt100", help="Send a VT100 chunk for an existing session")
    pv.add_argument("--sid", type=int, required=True)
    pv.add_argument("--text", default=None, help="Plain text; CRLF will be added")
    pv.add_argument("--raw", default=None, help="Raw escape bytes (e.g. '\\x1b[2J')")

    sub.add_parser("tail-screen", help="Tail the screen output file")

    pws = sub.add_parser(
        "window-switch",
        help="Emit CMD_WINDOW_SWITCH (relative): -1 prev, +1 next, 0 noop",
    )
    pws.add_argument("--delta", type=int, default=1)

    pwa = sub.add_parser(
        "window-activate",
        help="Emit CMD_WINDOW_ACTIVATE for a specific session id",
    )
    pwa.add_argument("--sid", type=int, required=True)

    pus = sub.add_parser(
        "set-ui-scale",
        help="Emit CMD_UI_SCALE_CHANGE; payload is [u8 cell_w, u8 cell_h]",
    )
    pus.add_argument("--cell-w", type=int, required=True,
                     help="Terminal cell width in pixels (e.g. 8/10/12/16)")
    pus.add_argument("--cell-h", type=int, required=True,
                     help="Terminal cell height in pixels (e.g. 16/20/24/32)")

    pw = sub.add_parser("windows", help="Windows native product-mode tools")
    wsub = pw.add_subparsers(dest="windows_cmd", required=True)
    wsub.add_parser("doctor", help="Check native Windows product-mode readiness")
    wsub.add_parser("plan", help="Print the Windows session adapter plan")
    pwd = wsub.add_parser("daemon", help="Run Windows product daemon on loopback TCP")
    pwd.add_argument("--state", default=None)
    pwd.add_argument("--screen", default=None)
    pwd.add_argument("--host", default="127.0.0.1")
    pwd.add_argument("--port", type=int, default=8765)
    pwd.add_argument(
        "--device",
        default="auto",
        help="Windows HID device path, 'auto', or 'none' for IPC-only development",
    )
    pwc = wsub.add_parser("cli", help="Run a Windows CLI under ConPTY and register a session")
    pwc.add_argument("--plugin", default=None, help="Session/plugin label (default: command basename)")
    pwc.add_argument("--ipc", default="tcp://127.0.0.1:8765", help="Windows daemon IPC endpoint")
    pwc.add_argument("argv", nargs=argparse.REMAINDER, help="Command to run, e.g. codex")

    pww = wsub.add_parser("wsl-cli", help="Run wsl.exe under ConPTY and register a session")
    pww.add_argument("--plugin", default="wsl-cli", help="Session/plugin label")
    pww.add_argument("--ipc", default="tcp://127.0.0.1:8765", help="Windows daemon IPC endpoint")
    pww.add_argument("--distro", default=None, help="WSL distribution name passed to wsl.exe -d")
    pww.add_argument("--wsl-cwd", default=None, help="Directory passed to wsl.exe --cd")
    pww.add_argument("argv", nargs=argparse.REMAINDER, help="Command after wsl.exe --, e.g. codex")

    pwr = wsub.add_parser("run", help="Compatibility alias for `windows cli`")
    pwr.add_argument("--plugin", default=None, help="Session/plugin label (default: command basename)")
    pwr.add_argument("--ipc", default="tcp://127.0.0.1:8765", help="Windows daemon IPC endpoint")
    pwr.add_argument("argv", nargs=argparse.REMAINDER, help="Command to run, e.g. codex")

    ph = sub.add_parser("hid", help="Real /dev/hidraw* probing tools")
    hsub = ph.add_subparsers(dest="hid_cmd", required=True)

    hsub.add_parser("list", help="Enumerate /dev/hidraw* with VID/PID and permissions")

    pp = hsub.add_parser(
        "probe",
        help="Open a hidraw device and report read/write capability + any pending input",
    )
    pp.add_argument("--device", required=True)
    pp.add_argument("--read-timeout", type=float, default=0.2)

    pH = hsub.add_parser(
        "handshake",
        help="Send CMD_REQUEST_SESSION and classify the response: "
        "PASS (new firmware), TIMEOUT (likely legacy firmware) or other",
    )
    pH.add_argument("--device", required=True)
    pH.add_argument("--plugin", default="hid-probe")
    pH.add_argument("--timeout", type=float, default=2.0)

    return p


def _setup_logging(verbose: int) -> None:
    level = logging.WARNING - 10 * min(verbose, 2)
    logging.basicConfig(level=level, format="%(asctime)s %(name)s %(levelname)s %(message)s")


def cmd_daemon(args: argparse.Namespace) -> int:
    hid_transport = None
    hid_path = args.hidraw
    if args.winhid is not None:
        if args.hidraw is not None:
            print("daemon: --hidraw and --winhid are mutually exclusive", file=sys.stderr)
            return 2
        from .transport_win_hid import WinHidTransport, resolve_win_hid_device

        hid_path = resolve_win_hid_device() if args.winhid == "auto" else args.winhid
        if not hid_path:
            print("daemon: no Windows Vibe HID device found", file=sys.stderr)
            return 1
        hid_transport = WinHidTransport(hid_path)
    cfg = DaemonConfig(
        sock_path=args.sock,
        state_path=args.state,
        max_sessions=args.max_sessions,
        hidraw_path=hid_path,
        hid_transport=hid_transport,
        hid_mode="real-winhid" if args.winhid is not None else "real-hidraw",
    )
    if args.screen is not None:
        cfg.screen_path = args.screen
    daemon = Daemon(cfg)
    daemon.run_forever()
    return 0


def cmd_ensure_daemon(args: argparse.Namespace) -> int:
    ok = ensure_daemon_running(
        args.sock,
        state_path=args.state,
        timeout=args.timeout,
        log_path=args.log,
    )
    if not os.path.exists(args.log):
        open(args.log, "ab").close()
    print(f"daemon socket : {args.sock}")
    print(f"socket status : {'reachable' if ok else 'unreachable'}")
    print(f"daemon log    : {args.log}")
    return 0 if ok else 1


def _load_state(path: str) -> Optional[dict]:
    try:
        with open(path, "r", encoding="utf-8") as f:
            state = json.load(f)
    except (OSError, json.JSONDecodeError):
        return None
    return state if isinstance(state, dict) else None


def _repo_root() -> str:
    return os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def _find_cli_behind_wrapper(name: str, wrapper_real: str) -> Optional[str]:
    seen = set()
    for raw in os.environ.get("PATH", "").split(os.pathsep):
        directory = os.path.expanduser(raw) if raw else "."
        if directory in seen:
            continue
        seen.add(directory)
        candidate = os.path.join(directory, name)
        if not os.path.isfile(candidate) or not os.access(candidate, os.X_OK):
            continue
        if os.path.realpath(candidate) == wrapper_real:
            continue
        return candidate
    return None


def _probe_socket(sock_path: str, *, timeout: float = 0.2) -> tuple[bool, Optional[str]]:
    if not is_tcp_endpoint(sock_path) and not os.path.exists(sock_path):
        return False, "socket file is missing"
    s: Optional[socket.socket] = None
    try:
        s = connect_packet_socket(sock_path, timeout=timeout)
        return True, None
    except PermissionError as exc:
        return False, f"permission denied while probing socket: {exc}"
    except (OSError, socket.timeout) as exc:
        return False, str(exc)
    finally:
        if s is not None:
            s.close()


def cmd_doctor(args: argparse.Namespace) -> int:
    fail = 0
    use_color = args.color == "always" or (
        args.color == "auto"
        and sys.stdout.isatty()
        and os.environ.get("NO_COLOR") is None
    )
    colors = {
        "OK": "\033[32m",
        "WARN": "\033[33m",
        "FAIL": "\033[31m",
        "RESET": "\033[0m",
    }

    def line(status: str, message: str) -> None:
        nonlocal fail
        label = f"[{status}]"
        if use_color:
            label = f"{colors[status]}{label}{colors['RESET']}"
        print(f"{label} {message}")
        if status == "FAIL":
            fail += 1

    print("vibe-bridge doctor")
    print()

    state = _load_state(args.state)
    state_sock = args.sock
    if state is not None and isinstance(state.get("sock_path"), str):
        state_sock = state["sock_path"]
    socket_ok, socket_reason = _probe_socket(state_sock)
    if socket_ok:
        line("OK", f"daemon socket reachable: {state_sock}")
    else:
        detail = f" ({socket_reason})" if socket_reason else ""
        line("WARN", f"daemon socket not reachable: {state_sock}{detail}")
        line("OK", "recovery command: python3 -m vibe_bridge.main ensure-daemon")

    if state is None:
        line("WARN", f"state file missing or unreadable: {args.state}")
    else:
        mode = state.get("mode") or "unknown"
        sock_path = state.get("sock_path") or args.sock
        line("OK", f"state file readable: mode={mode} sock={sock_path}")
        try:
            age = max(0.0, time.time() - os.path.getmtime(args.state))
            if age > 30.0 and not socket_ok:
                line("WARN", f"state is stale: age={age:.1f}s")
            else:
                line("OK", f"state age: {age:.1f}s")
        except OSError:
            pass
        hidraw_path = state.get("hidraw_path")
        if hidraw_path:
            if os.path.exists(str(hidraw_path)):
                line("OK", f"daemon hidraw path exists: {hidraw_path}")
            else:
                line("FAIL", f"daemon hidraw path is gone: {hidraw_path}")

    try:
        from .transport_hidraw import list_hidraw_devices

        devices = list_hidraw_devices()
    except Exception as exc:
        devices = []
        line("WARN", f"hidraw enumeration failed: {exc}")

    expected = f"{VIBE_USB_VID:04x}:{VIBE_USB_PID:04x}"
    vibe_devices = [d for d in devices if d.vid == VIBE_USB_VID and d.pid == VIBE_USB_PID]
    if vibe_devices:
        for d in vibe_devices:
            rw = "rw" if d.readable and d.writable else (
                ("r" if d.readable else "-") + ("w" if d.writable else "-")
            )
            status = "OK" if d.readable and d.writable else "FAIL"
            line(status, f"Vibe HID {expected} at {d.path} permissions={rw}")
    else:
        line("WARN", f"no Vibe HID device ({expected}) detected")

    auto_hid = resolve_hidraw_device()
    if auto_hid:
        line("OK", f"automatic hidraw selection: {auto_hid}")
    else:
        line("OK", "automatic hidraw selection is disabled until a Vibe VID:PID is present")

    other_hid = [d for d in devices if d not in vibe_devices]
    if other_hid:
        line("OK", f"non-Vibe hidraw devices ignored by auto-selection: {len(other_hid)}")

    root = _repo_root()
    cli_names = args.cli or ["codex", "claude"]
    for name in cli_names:
        wrapper_path = os.path.join(root, "bin", name)
        wrapper_real = os.path.realpath(wrapper_path)
        found = shutil.which(name)
        if not os.path.exists(wrapper_path):
            line("WARN", f"{name}: wrapper missing at {wrapper_path}")
            continue
        if found is None:
            line("WARN", f"{name}: not found on PATH")
            continue
        if os.path.realpath(found) == wrapper_real:
            line("OK", f"{name}: wrapper active at {found}")
            real_cli = _find_cli_behind_wrapper(name, wrapper_real)
            if real_cli is None:
                line("FAIL", f"{name}: no real CLI found behind wrapper on PATH")
            else:
                line("OK", f"{name}: real CLI candidate behind wrapper: {real_cli}")
        else:
            line("WARN", f"{name}: PATH resolves to non-wrapper binary: {found}")

    if os.path.exists(args.log):
        line("OK", f"daemon log exists: {args.log}")
    else:
        line("WARN", f"daemon log missing: {args.log}")

    print()
    if fail:
        print("doctor result: FAIL")
        return 1
    print("doctor result: OK with warnings" if state is None or not vibe_devices else "doctor result: OK")
    return 0


def cmd_sessions(args: argparse.Namespace) -> int:
    try:
        with open(args.state, "r", encoding="utf-8") as f:
            state = json.load(f)
    except FileNotFoundError:
        print(f"no state file at {args.state} (is the daemon running?)", file=sys.stderr)
        return 1
    sock_path = state.get("sock_path") or args.sock
    reachable = can_connect(sock_path) if isinstance(sock_path, str) else False
    print(f"daemon socket : {sock_path}")
    print(f"socket status : {'reachable' if reachable else 'unreachable/stale'}")
    try:
        age = max(0.0, time.time() - os.path.getmtime(args.state))
        print(f"state age     : {age:.1f}s")
    except OSError:
        pass
    if state.get("mode") is not None:
        print(f"mode          : {state.get('mode')}")
    if state.get("hidraw_path"):
        print(f"hidraw        : {state.get('hidraw_path')}")
    focused_sid = state.get("focused_sid")
    if focused_sid is None:
        focused_sid = state.get("active_sid")
    print(f"focused sid   : {focused_sid}")
    print(f"board panel   : {state.get('board_panel')}")
    if state.get("selector_focus_sid") is not None:
        print(f"selector sid  : {state.get('selector_focus_sid')}")
    if state.get("terminal_visible") is not None:
        print(f"terminal view : {state.get('terminal_visible')}")
    if state.get("last_board_event"):
        print(f"last input    : {state.get('last_board_event')}")
        decoded = _describe_last_board_event(str(state.get("last_board_event")))
        if decoded:
            print(f"input decoded : {decoded}")
        if state.get("last_board_event_seq") is not None:
            print(f"input seq     : {state.get('last_board_event_seq')}")
    if state.get("last_board_tx"):
        print(f"last board tx : {state.get('last_board_tx')}")
    hooked_agents = state.get("hooked_agents", [])
    print(f"hooked agents : {len(hooked_agents)}")
    for agent in hooked_agents:
        print(
            f"  pid={agent.get('pid')}  kind={agent.get('kind')}  "
            f"sid={agent.get('sid')}"
        )
    print(f"sessions ({len(state.get('sessions', []))}):")
    for s in state.get("sessions", []):
        ctx = s.get("context", {}) or {}
        ctx_short = ", ".join(f"{k}={v}" for k, v in ctx.items())
        print(
            f"  sid={s['sid']:>4}  plugin={s['plugin']:<24}  "
            f"buf={state['buffers'].get(str(s['sid']), 0)}b  ctx={{{ctx_short}}}"
        )
    return 0


def _describe_last_board_event(event: str) -> str:
    if not event.startswith("key bits=0x"):
        return ""
    parts = event.split()
    try:
        bits = int(parts[1].split("=", 1)[1], 16)
    except (IndexError, ValueError):
        return ""
    enc_pressed = "enc=1" in parts
    names = [
        ("REJECT", 0),
        ("VOICE", 1),
        ("SESSION", 2),
        ("VOTE_REVIEW", 3),
        ("AGENT_MODEL", 4),
        ("MULTI_FUNCTION", 5),
        ("CONFIRM", 6),
        ("MENU_DEBUG", 7),
    ]
    pressed = [name for name, bit in names if bits & (1 << bit)]
    if enc_pressed:
        pressed.append("ENCODER_PRESS")
    return ", ".join(pressed) if pressed else "none"


def _await_response(client: MockHidClient, *, timeout: float = 2.0) -> Optional[int]:
    pkt = client.recv_packet(timeout=timeout)
    if pkt is None:
        print("no response from daemon (timed out)", file=sys.stderr)
        return None
    if pkt.command == int(Cmd.SESSION_RESPONSE):
        status = Status(pkt.payload[0]) if pkt.payload else Status.OK
        print(f"session created: sid={pkt.session_id} status={status.name}")
        return pkt.session_id
    if pkt.command == int(Cmd.SESSION_INVALID):
        status = Status(pkt.payload[0]) if pkt.payload else Status.INVALID
        print(f"session invalid: status={status.name}", file=sys.stderr)
        return None
    print(f"unexpected response: cmd=0x{pkt.command:02x} sid={pkt.session_id}", file=sys.stderr)
    return None


def _format_report_bytes(raw: bytes) -> str:
    trimmed = raw.rstrip(b"\x00")
    if not trimmed and raw:
        trimmed = raw[:1]
    pad = len(raw) - len(trimmed)
    suffix = f" (+{pad} zero pad)" if pad else ""
    return f"{trimmed.hex(' ')}{suffix}"


def _valid_session_response(pkt: Packet) -> Optional[Status]:
    if pkt.report_id != int(ReportId.HOST_BOUND):
        return None
    if pkt.command != int(Cmd.SESSION_RESPONSE):
        return None
    if pkt.session_id == SESSION_BROADCAST:
        return None
    if len(pkt.payload) != 1:
        return None
    try:
        return Status(pkt.payload[0])
    except ValueError:
        return None


def cmd_request_session(args: argparse.Namespace) -> int:
    client = MockHidClient(args.sock)
    try:
        print("requesting session id...")
        client.send_packet(make_request_session(hint=args.plugin.encode("utf-8")))
        sid = _await_response(client)
        return 0 if sid is not None else 1
    finally:
        client.close()


def cmd_send_vt100(args: argparse.Namespace) -> int:
    if args.text is None and args.raw is None:
        print("send-vt100 requires --text or --raw", file=sys.stderr)
        return 2
    if args.text is not None:
        payload = (args.text + "\r\n").encode("utf-8")
    else:
        # interpret simple python-style escape sequences
        payload = args.raw.encode("utf-8").decode("unicode_escape").encode("latin-1")

    client = MockHidClient(args.sock)
    try:
        for pkt in stream_iter_packets(args.sid, payload):
            client.send_packet(pkt)
        print(f"sent {len(payload)} bytes to sid {args.sid}")
        return 0
    finally:
        client.close()


def cmd_window_switch(args: argparse.Namespace) -> int:
    delta = max(-128, min(127, int(args.delta))) & 0xFF
    pkt = Packet(
        report_id=int(ReportId.HOST_BOUND),
        command=int(Cmd.WINDOW_SWITCH),
        session_id=SESSION_BROADCAST,
        payload=bytes([delta]),
    )
    client = MockHidClient(args.sock)
    try:
        client.send_packet(pkt)
        print(f"window-switch delta={args.delta} sent")
        return 0
    finally:
        client.close()


def cmd_window_activate(args: argparse.Namespace) -> int:
    pkt = Packet(
        report_id=int(ReportId.HOST_BOUND),
        command=int(Cmd.WINDOW_ACTIVATE),
        session_id=int(args.sid),
        payload=b"",
    )
    client = MockHidClient(args.sock)
    try:
        client.send_packet(pkt)
        print(f"window-activate sid={args.sid} sent")
        return 0
    finally:
        client.close()


def cmd_set_ui_scale(args: argparse.Namespace) -> int:
    if not (1 <= int(args.cell_w) <= 255) or not (1 <= int(args.cell_h) <= 255):
        print("set-ui-scale: cell_w/cell_h must be in 1..255", file=sys.stderr)
        return 2
    pkt = Packet(
        report_id=int(ReportId.HOST_BOUND),
        command=int(Cmd.UI_SCALE_CHANGE),
        session_id=SESSION_BROADCAST,
        payload=bytes([int(args.cell_w) & 0xFF, int(args.cell_h) & 0xFF]),
    )
    client = MockHidClient(args.sock)
    try:
        client.send_packet(pkt)
        print(f"set-ui-scale {args.cell_w}x{args.cell_h} sent")
        return 0
    finally:
        client.close()


def cmd_hid_list(args: argparse.Namespace) -> int:
    from .transport_hidraw import list_hidraw_devices

    devs = list_hidraw_devices()
    if not devs:
        print("no /dev/hidraw* devices found", file=sys.stderr)
        return 1
    print(f"{'device':<18} {'vid:pid':<10} r w  uevent")
    for d in devs:
        rw = ("r" if d.readable else "-") + ("w" if d.writable else "-")
        uevent_first = (d.name or "").splitlines()[0] if d.name else ""
        print(f"{d.path:<18} {d.vid_pid_str():<10} {rw:<3} {uevent_first}")
    return 0


def cmd_hid_probe(args: argparse.Namespace) -> int:
    from .transport_hidraw import HidrawTransport

    print(f"opening {args.device} ...")
    if not os.access(args.device, os.R_OK | os.W_OK):
        print(
            f"  WARN: missing rw permission on {args.device} "
            "(run `sudo chmod 666 {args.device}` or add a udev rule)",
            file=sys.stderr,
        )
    try:
        t = HidrawTransport(args.device)
    except OSError as exc:
        print(f"  open failed: {exc}", file=sys.stderr)
        return 2
    try:
        print(f"  opened, report_length={t.report_length}")
        print(f"  draining input for {args.read_timeout}s ...")
        n = 0
        while True:
            raw = t.recv_report(timeout=args.read_timeout if n == 0 else 0.05)
            if raw is None:
                break
            n += 1
            try:
                pkt = Packet.decode(raw)
                print(
                    f"    pkt#{n}: report_id=0x{pkt.report_id:02x} "
                    f"cmd=0x{pkt.command:02x} sid={pkt.session_id} "
                    f"payload={len(pkt.payload)}b raw={_format_report_bytes(raw)}"
                )
            except ProtocolError as exc:
                print(
                    f"    pkt#{n}: decode_error={exc} "
                    f"raw={_format_report_bytes(raw)}"
                )
            if n >= 16:
                print("    (more pending; capped at 16)")
                break
        print(f"  drained {n} packet(s)")
        return 0
    finally:
        t.close()


def cmd_hid_handshake(args: argparse.Namespace) -> int:
    from .transport_hidraw import HidrawTransport
    import time

    try:
        t = HidrawTransport(args.device)
    except OSError as exc:
        print(f"open failed: {exc}", file=sys.stderr)
        return 2
    try:
        # Drain any stale input first so we don't mistake old key/encoder reports
        # for our handshake reply.
        while True:
            stale = t.recv_report(timeout=0.05)
            if stale is None:
                break

        req = make_request_session(hint=args.plugin.encode("utf-8"))
        print(
            f"sending CMD_REQUEST_SESSION (report=0x{req.report_id:02x}, "
            f"hint={args.plugin!r})"
        )
        t.send_packet(req)

        # Collect everything that comes back within the timeout window so we
        # can tell PASS vs LEGACY_NOISE vs TIMEOUT apart.
        collected = []
        deadline = time.time() + args.timeout
        while True:
            remaining = deadline - time.time()
            if remaining <= 0:
                break
            raw = t.recv_report(timeout=min(remaining, 0.5))
            if raw is None:
                continue
            try:
                pkt = Packet.decode(raw)
            except ProtocolError as exc:
                collected.append((None, raw, str(exc)))
                continue
            status = _valid_session_response(pkt)
            if status is not None:
                print(
                    f"RESULT=PASS  sid={pkt.session_id} status={status.name} "
                    f"(board speaks new protocol)"
                )
                return 0
            collected.append((pkt, raw, None))

        if collected:
            print(
                f"RESULT=LEGACY_NOISE  no valid SESSION_RESPONSE in {args.timeout}s, "
                "but board sent these reports back:"
            )
            for pkt, raw, err in collected:
                if pkt is None:
                    print(f"  decode_error={err} raw={_format_report_bytes(raw)}")
                else:
                    print(
                        f"  report_id=0x{pkt.report_id:02x} "
                        f"cmd=0x{pkt.command:02x} sid={pkt.session_id} "
                        f"payload={pkt.payload!r} raw={_format_report_bytes(raw)}"
                    )
            print(
                "(consistent with legacy aikb_hid_input firmware that doesn't "
                "understand the new packet header — proceed with firmware upgrade)"
            )
            return 1
        print(
            f"RESULT=TIMEOUT  no reply within {args.timeout}s "
            "(device unreachable or firmware silent)"
        )
        return 1
    finally:
        t.close()


def cmd_tail_screen(args: argparse.Namespace) -> int:
    path = "/tmp/vibe-bridge-screen.out"
    try:
        with open(path, "rb") as f:
            sys.stdout.buffer.write(f.read())
            sys.stdout.buffer.flush()
        return 0
    except FileNotFoundError:
        print(f"no screen file at {path}", file=sys.stderr)
        return 1


def cmd_windows_doctor(args: argparse.Namespace) -> int:
    from .windows_host import doctor_checks, has_failures

    checks = doctor_checks()
    print("vibe-bridge windows doctor")
    print()
    for check in checks:
        print(f"[{check.status}] {check.name}: {check.detail}")
    print()
    if has_failures(checks):
        print("windows product result: NOT READY")
        return 1
    print("windows product result: READY")
    return 0


def cmd_windows_plan(args: argparse.Namespace) -> int:
    from .windows_host import adapter_plan

    print("vibe-bridge windows product plan")
    print()
    print("Protocol: keep the existing board-assigned session_id, HID packets, VT100 stream,")
    print("WINDOW_ACTIVATE, key events, and encoder events unchanged.")
    print()
    for adapter in adapter_plan():
        print(f"- {adapter.name} [{adapter.status}]")
        print(f"  source     : {adapter.source}")
        print(f"  session    : {adapter.session_path}")
        print(f"  activation : {adapter.activation_path}")
    return 0


def cmd_windows_daemon(args: argparse.Namespace) -> int:
    from .windows_host import default_screen_path, default_state_path

    endpoint = f"tcp://{args.host}:{args.port}"
    state_path = args.state or default_state_path()
    screen_path = args.screen or default_screen_path()
    os.makedirs(os.path.dirname(state_path), exist_ok=True)
    os.makedirs(os.path.dirname(screen_path), exist_ok=True)
    device = args.device
    hid_transport = None
    hid_path = None
    if device != "none":
        from .transport_win_hid import WinHidTransport, resolve_win_hid_device

        hid_path = resolve_win_hid_device() if device == "auto" else device
        if not hid_path:
            print("windows daemon: no Vibe HID 359f:2120 device found", file=sys.stderr)
            return 1
        hid_transport = WinHidTransport(hid_path)

    cfg = DaemonConfig(
        sock_path=endpoint,
        state_path=state_path,
        screen_path=screen_path,
        hidraw_path=hid_path,
        hid_transport=hid_transport,
        hid_mode="real-winhid",
    )
    print(f"windows daemon ipc : {endpoint}")
    print(f"windows daemon hid : {hid_path or 'disabled'}")
    print(f"windows daemon state : {state_path}")
    daemon = Daemon(cfg)
    daemon.run_forever()
    return 0


def cmd_windows_run(args: argparse.Namespace) -> int:
    from .windows_runner import run_windows_cli

    return run_windows_cli(args.argv, sock_path=args.ipc, plugin_name=args.plugin)


def cmd_windows_wsl_cli(args: argparse.Namespace) -> int:
    from .windows_runner import run_wsl_cli

    return run_wsl_cli(
        args.argv,
        sock_path=args.ipc,
        plugin_name=args.plugin,
        distro=args.distro,
        wsl_cwd=args.wsl_cwd,
    )


def main(argv: Optional[list] = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)
    _setup_logging(args.verbose)

    dispatch = {
        "daemon": cmd_daemon,
        "ensure-daemon": cmd_ensure_daemon,
        "doctor": cmd_doctor,
        "sessions": cmd_sessions,
        "request-session": cmd_request_session,
        "send-vt100": cmd_send_vt100,
        "tail-screen": cmd_tail_screen,
        "window-switch": cmd_window_switch,
        "window-activate": cmd_window_activate,
        "set-ui-scale": cmd_set_ui_scale,
    }
    if args.cmd == "hid":
        hid_dispatch = {
            "list": cmd_hid_list,
            "probe": cmd_hid_probe,
            "handshake": cmd_hid_handshake,
        }
        return hid_dispatch[args.hid_cmd](args)
    if args.cmd == "windows":
        windows_dispatch = {
            "doctor": cmd_windows_doctor,
            "plan": cmd_windows_plan,
            "daemon": cmd_windows_daemon,
            "cli": cmd_windows_run,
            "wsl-cli": cmd_windows_wsl_cli,
            "run": cmd_windows_run,
        }
        return windows_dispatch[args.windows_cmd](args)
    handler = dispatch[args.cmd]
    return handler(args)


if __name__ == "__main__":
    raise SystemExit(main())
