"""Hello-world plugin for vibe-bridge.

Run after the daemon is up::

    python3 -m vibe_bridge.main daemon &       # in another shell
    python3 plugins/terminal_demo/main.py

The plugin requests a session id, then writes a small VT100 banner to its own
session. Watch ``/tmp/vibe-bridge-screen.out`` (or ``vibe-bridge tail-screen``)
to see the bytes that would land on the LCD.
"""

from __future__ import annotations

import os
import sys
import time

# Allow running the script directly without `pip install`.
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "src"))

from vibe_bridge.plugin_client import PluginClient  # noqa: E402


VT_CLEAR = "\x1b[2J\x1b[H"
VT_BOLD = "\x1b[1m"
VT_RESET = "\x1b[0m"


def main() -> int:
    print("requesting session id...")
    with PluginClient(plugin_name="terminal_demo") as p:
        sid = p.acquire_session(timeout=2.0)
        print(f"session created: {sid}")

        banner = (
            VT_CLEAR
            + f"{VT_BOLD}vibe-bridge terminal_demo{VT_RESET}\r\n"
            + f"  sid    : {sid}\r\n"
            + f"  pid    : {os.getpid()}\r\n"
            + f"  cwd    : {os.getcwd()}\r\n"
            + f"  ts     : {time.strftime('%H:%M:%S')}\r\n"
            + "----------------------------------------\r\n"
        )
        p.send_vt100(banner)

        for i in range(1, 6):
            p.send_vt100(f"line {i}: hello from sid {sid}\r\n")
            time.sleep(0.1)
    print("done")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
