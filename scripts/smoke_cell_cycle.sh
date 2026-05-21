#!/usr/bin/env bash
# End-to-end smoke test for the host->board font-size (cell preset) link.
#
# Verifies the full chain after burning the 2026-05-08 firmware (or later):
#   host CLI -> vibe-bridge daemon -> hidraw -> aikb_hid_input
#     -> /tmp/aikb_lcd_ui.ctrl FIFO -> aikb_lcd_ui apply_cell_size -> LCD
#
# Run on the WSL/Linux host with the board attached. On WSL, forward the
# 359f:2120 USB bus-id into WSL first via `usbipd attach --wsl --busid <ID>`.
#
# What it does:
#   1. Locates the Vibe 359f:2120 hidraw node (override with VIBE_HIDRAW_DEVICE).
#   2. Ensures a vibe-bridge daemon is running and bound to that node;
#      restarts if a stale daemon is bound to a different/dead node.
#   3. Within a single long-lived PluginClient (so the sid stays alive),
#      acquires a sid and cycles through 4 cell-size presets, sending a
#      VT100 banner at each step.
#
# Eyeball the LCD: you should see four banners, each with a clearly
# different font size (8x16, 10x20, 12x24, 16x32).
#
# Why a single PluginClient: every short-lived CLI (`request-session`,
# `send-vt100`) closes its socket on exit, which makes the daemon
# release the sid. Stitching them together in shell will fail validation.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/src"
LOG="${VIBE_DAEMON_LOG:-/tmp/vibe-daemon.log}"

# --- 1. locate hidraw node ---------------------------------------------------

DEV="${VIBE_HIDRAW_DEVICE:-}"
if [ -z "$DEV" ]; then
    DEV="$(PYTHONPATH="$SRC" python3 -m vibe_bridge.main hid list 2>/dev/null \
        | awk '$2 == "359f:2120" {print $1; exit}')"
fi
if [ -z "$DEV" ] || [ ! -c "$DEV" ]; then
    cat >&2 <<EOF
ERROR: no Vibe 359f:2120 /dev/hidraw* node found.
  - On WSL: run 'usbipd attach --wsl --busid <359f:2120 BUSID>' in
    PowerShell, then re-run this script.
  - Or set VIBE_HIDRAW_DEVICE to override autodetection.
EOF
    exit 2
fi
echo "[*] using $DEV"

# --- 2. ensure daemon --------------------------------------------------------

current_dev() {
    PYTHONPATH="$SRC" python3 -m vibe_bridge.main sessions 2>/dev/null \
        | awk -F'[[:space:]]*:[[:space:]]*' '/^hidraw/ {print $2; exit}'
}

start_daemon() {
    PYTHONPATH="$SRC" nohup python3 -m vibe_bridge.main daemon --hidraw "$DEV" \
        > "$LOG" 2>&1 &
    disown
    sleep 1
}

if pgrep -f 'vibe_bridge.main.*daemon' >/dev/null 2>&1; then
    bound="$(current_dev || true)"
    if [ "$bound" = "$DEV" ]; then
        echo "[*] reusing existing daemon on $DEV"
    else
        echo "[*] stale daemon bound to '${bound:-?}', restarting..."
        pkill -f 'vibe_bridge.main.*daemon' || true
        sleep 0.5
        start_daemon
    fi
else
    echo "[*] starting daemon"
    start_daemon
fi

if [ "$(current_dev || true)" != "$DEV" ]; then
    echo "ERROR: daemon failed to bind to $DEV — see $LOG" >&2
    tail -n 20 "$LOG" >&2 || true
    exit 3
fi
echo "[*] daemon ready on $DEV"

# --- 3. cell-size cycle (single PluginClient context) ------------------------

PYTHONPATH="$SRC" python3 - <<'PY'
import time
from vibe_bridge.plugin_client import PluginClient
from vibe_bridge.hid_protocol import Packet, Cmd, ReportId, SESSION_BROADCAST

CELLS = [(8, 16), (10, 20), (12, 24), (16, 32)]

with PluginClient(plugin_name="cell_cycle_smoke") as p:
    sid = p.acquire_session(timeout=2.0)
    print(f"[*] acquired sid={sid}", flush=True)
    p.send_vt100(
        b"\x1b[2J\x1b[H\x1b[1;33mhello sid=" + str(sid).encode()
        + b"\x1b[0m\r\n"
    )
    time.sleep(2)
    for w, h in CELLS:
        pkt = Packet(
            report_id=ReportId.DEVICE_BOUND,
            command=Cmd.UI_SCALE_CHANGE,
            session_id=SESSION_BROADCAST,
            payload=bytes([w, h]),
        )
        p.send_packet(pkt)
        time.sleep(0.4)
        banner = (
            f"\x1b[2J\x1b[H\x1b[1;33mcell {w}x{h}\x1b[0m\r\n"
            f"  hello sid={sid}\r\n"
        )
        p.send_vt100(banner.encode())
        print(f"[*] sent cell {w}x{h}", flush=True)
        time.sleep(3)
    print("[*] hold 3s before release", flush=True)
    time.sleep(3)
PY

cat <<EOF

[*] done. Eyeball the LCD: four banners, font growing 8x16 -> 16x32.
    If anything's off:
      tail -30 $LOG
      on board: tail -30 /tmp/aikb_hid_input.log /tmp/aikb_lcd_ui.log
EOF
