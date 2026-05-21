#!/usr/bin/env bash
# Probe the real /dev/hidraw* device exposed by the Vibe HID gadget.
#
# Run this on the WSL/Linux host with the board attached (and its USB
# bus-id forwarded into WSL via usbipd if applicable).
#
# Three checks:
#   1. `hid list`       — enumerate every hidraw node and its VID:PID. The
#                         Vibe gadget is `359f:2120`.
#   2. `hid probe`      — open the device, drain any pending input. Tells us
#                         the wrapper can read/write the node and surfaces any
#                         status/event reports the board is already emitting.
#   3. `hid handshake`  — send CMD_REQUEST_SESSION (new protocol). Classifies:
#        PASS    → board firmware already speaks the new protocol.
#        TIMEOUT → legacy firmware: it ignores the new header (expected at
#                  this stage, before the aikb_hid_input upgrade).
#        OTHER   → board responded with something unexpected; capture the
#                  payload bytes for diagnosis.
#
# WHAT TO SEND BACK
# -----------------
# Copy the entire script output to the chat. The interesting lines are:
#   - the `hid list` row matching 359f:2120 (or whichever VID:PID the firmware
#     uses if it changed),
#   - the permission warning (if any),
#   - any pkt# entries from `probe`,
#   - the RESULT= line from `handshake`.

set -u

REPO_DIR="${REPO_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

cd "$REPO_DIR"

DEVICE="${1:-}"
if [ -z "$DEVICE" ]; then
    DEVICE="$(PYTHONPATH=src python3 -m vibe_bridge.main hid list 2>/dev/null \
        | awk '$2 == "359f:2120" {print $1; exit}')"
fi

echo "============================================================"
echo "  vibe-bridge real-HID probe"
echo "  repo:   $REPO_DIR"
echo "  device: $DEVICE"
echo "  date:   $(date -Iseconds)"
echo "============================================================"
echo

echo "--- 1. hid list ---"
PYTHONPATH=src python3 -m vibe_bridge.main hid list || true
echo

echo "--- /dev/hidraw* permissions ---"
ls -l /dev/hidraw* 2>&1 || true
echo

if [ -z "$DEVICE" ] || [ ! -e "$DEVICE" ]; then
    echo "Vibe 359f:2120 hidraw device not found; aborting probe/handshake."
    echo "Reattach via:  usbipd attach --wsl --busid 8-1   (Windows host)"
    exit 1
fi

echo "--- 2. hid probe (drain 0.5s of input) ---"
PYTHONPATH=src python3 -m vibe_bridge.main hid probe \
    --device "$DEVICE" --read-timeout 0.5 || true
echo

echo "--- 3. hid handshake (CMD_REQUEST_SESSION, 2s timeout) ---"
PYTHONPATH=src python3 -m vibe_bridge.main hid handshake \
    --device "$DEVICE" --plugin probe-script --timeout 2.0
HRC=$?
echo
echo "handshake exit code: $HRC"
echo
echo "============================================================"
echo "  done."
echo "============================================================"
