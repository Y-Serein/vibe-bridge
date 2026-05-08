"""vibe-bridge CLI entry point."""

from __future__ import annotations

import argparse
import json
import logging
import os
import sys
from typing import Optional

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
    cfg = DaemonConfig(
        sock_path=args.sock,
        state_path=args.state,
        max_sessions=args.max_sessions,
        hidraw_path=args.hidraw,
    )
    if args.screen is not None:
        cfg.screen_path = args.screen
    daemon = Daemon(cfg)
    daemon.run_forever()
    return 0


def cmd_sessions(args: argparse.Namespace) -> int:
    try:
        with open(args.state, "r", encoding="utf-8") as f:
            state = json.load(f)
    except FileNotFoundError:
        print(f"no state file at {args.state} (is the daemon running?)", file=sys.stderr)
        return 1
    print(f"daemon socket : {state.get('sock_path')}")
    if state.get("mode") is not None:
        print(f"mode          : {state.get('mode')}")
    if state.get("hidraw_path"):
        print(f"hidraw        : {state.get('hidraw_path')}")
    print(f"active sid    : {state.get('active_sid')}")
    print(f"sessions ({len(state.get('sessions', []))}):")
    for s in state.get("sessions", []):
        ctx = s.get("context", {}) or {}
        ctx_short = ", ".join(f"{k}={v}" for k, v in ctx.items())
        print(
            f"  sid={s['sid']:>4}  plugin={s['plugin']:<24}  "
            f"buf={state['buffers'].get(str(s['sid']), 0)}b  ctx={{{ctx_short}}}"
        )
    return 0


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


def main(argv: Optional[list] = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)
    _setup_logging(args.verbose)

    dispatch = {
        "daemon": cmd_daemon,
        "sessions": cmd_sessions,
        "request-session": cmd_request_session,
        "send-vt100": cmd_send_vt100,
        "tail-screen": cmd_tail_screen,
        "window-switch": cmd_window_switch,
        "window-activate": cmd_window_activate,
    }
    if args.cmd == "hid":
        hid_dispatch = {
            "list": cmd_hid_list,
            "probe": cmd_hid_probe,
            "handshake": cmd_hid_handshake,
        }
        return hid_dispatch[args.hid_cmd](args)
    handler = dispatch[args.cmd]
    return handler(args)


if __name__ == "__main__":
    raise SystemExit(main())
