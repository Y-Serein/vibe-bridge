# vibe-bridge

Multi-window session router between CLI tools (Codex, Claude Code, OpenCode, ...)
and a HID screen. Plugins request a session id from the bridge daemon; every
subsequent packet — key/encoder events, VT100 output, status updates — carries
that session id, and the daemon routes per-session VT100 streams to independent
buffers. The currently active window is mirrored either to a mock screen file
or to a real `/dev/hidraw*` device.

This Python package supports both the mock Unix-socket path and the real hidraw
bridge path. In real mode, the daemon only opens/probes `/dev/hidraw*` at
startup; it forwards each plugin/wrapper `CMD_REQUEST_SESSION` to the board and
uses the board-returned `session_id` as authoritative.

See `request.md` for the full requirements specification.

---

## Layout

```
vibe-bridge/
  pyproject.toml
  src/vibe_bridge/
    hid_protocol.py    # packet codec, CMD_*, Status, fragmentation
    session_manager.py # uint16 sid pool, TTL, LRU, invalidation
    transport.py       # Transport abstract base
    mock_hid.py        # Unix-socket mock HID server + client
    vt100_router.py    # per-session VT100 buffers, active window
    daemon.py          # owns SessionManager + Vt100Router + MockHidServer
    plugin_client.py   # plugin SDK (acquire_session, send_vt100)
    main.py            # `vibe-bridge` CLI entry
  plugins/
    terminal_demo/     # hello-world plugin
  tests/               # stdlib unittest (no external deps)
  docs/                # protocol + architecture
```

---

## Quick start

The MVP requires only Python ≥ 3.9 (stdlib).

```bash
# Terminal 1: start the bridge
cd vibe-bridge
PYTHONPATH=src python3 -m vibe_bridge.main -vv daemon

# Terminal 2: run the demo plugin
cd vibe-bridge
PYTHONPATH=src python3 plugins/terminal_demo/main.py
# requesting session id...
# session created: 1
# done

# Inspect state and screen output
PYTHONPATH=src python3 -m vibe_bridge.main sessions
cat -v /tmp/vibe-bridge-screen.out
```

Default paths:

| Path                            | Purpose                                     |
| ------------------------------- | ------------------------------------------- |
| `/tmp/vibe-bridge.sock`         | Unix socket the daemon listens on            |
| `/tmp/vibe-bridge-state.json`   | Session table dump (read by `sessions` CLI) |
| `/tmp/vibe-bridge-screen.out`   | VT100 bytes for the **active** session only |

Override with `--sock`, `--state`, `--screen`.

---

## CLI

```
vibe-bridge daemon                              # foreground daemon
vibe-bridge daemon --hidraw /dev/hidraw0        # foreground real-HID bridge
vibe-bridge sessions                            # print session table
vibe-bridge request-session [--plugin NAME]     # smoke handshake test
vibe-bridge send-vt100 --sid N --text "..."     # send a VT100 chunk
vibe-bridge tail-screen                         # dump the screen file
vibe-bridge window-switch --delta {-1,0,1}      # rotate active window
vibe-bridge window-activate --sid N             # activate a specific session
```

---

## Tests

```bash
PYTHONPATH=src python3 -m unittest discover -s tests
```

57 tests, no external dependencies.

---

## Shell wrappers

`bin/codex` and `bin/claude` are drop-in wrappers. Put this repo's `bin/`
directory ahead of the real CLI on `$PATH`:

```bash
export PATH="$PWD/bin:$PATH"
codex
```

The wrapper:

1. Walks `$PATH` to find the real binary, skipping itself.
2. Calls `ensure_daemon_running` — spawns a detached daemon if the socket isn't
   there, auto-detecting the Vibe HID device (`VID:PID 359f:2120`) when present.
3. Acquires a fresh session id for each top-level wrapper run.
4. Exports `VIBE_SESSION_ID` and `VIBE_SOCK_PATH`; in an interactive TTY it
   runs the real CLI under a PTY and forwards VT100 output to the active window,
   while non-TTY invocations fall through to `execvp`.

Set `VIBE_BRIDGE_DISABLE=1` to bypass the bridge entirely (still execs the real
binary).

`VIBE_HIDRAW_DEVICE=/dev/hidrawN` and `VIBE_SOCK_PATH=/tmp/custom.sock` remain
available as overrides, but they are not needed for the normal `codex` path.
Set `VIBE_BRIDGE_REUSE_SESSION=1` only when an intentionally nested CLI should
reuse `$VIBE_SESSION_ID`; otherwise every `codex` run gets a new window.
When an interactive wrapper exits, the daemon releases that sid; if it was the
active window, the screen switches to another live window and replays its buffer.

## Status

- [x] HID packet codec with `session_id` in header (uint16, little-endian)
- [x] Session manager: alloc, TTL, LRU eviction, invalidation callback
- [x] Mock HID transport over Unix socket
- [x] Per-session VT100 buffers + single active window
- [x] Daemon + CLI + plugin SDK + hello-world plugin
- [x] Smoke test: two plugins, two sessions, no cross-contamination
- [x] Shell wrapper for `codex` / `claude` (auto-daemon, env injection)
- [x] Owner-rebind on existing-sid packets (so re-launched processes can re-claim)
- [x] PTY-capture forwarder: tee real CLI stdout into `CMD_VT100_STREAM`
- [x] WINDOW_SWITCH / WINDOW_ACTIVATE: replay buffer with screen-clear on switch
- [x] `HidrawTransport` + `vibe-bridge hid {list,probe,handshake}` probe CLI
- [x] Board-side `aikb_hid_input.c` upgraded to v0 protocol and packaged into rootfs image
- [x] Wire `HidrawTransport` into daemon (`daemon --hidraw /dev/hidraw0`)
- [x] Daemon handlers for `CMD_KEY_EVENT` / `CMD_ENCODER_EVENT` (key routes to active owner; encoder switches active window)
- [ ] Board-side `aikb_lcd_ui` multi-buffer (only if single FIFO turns out to cross-talk)

### Forward modes

| `VIBE_BRIDGE_FORWARD` | Behaviour                                                |
| --------------------- | -------------------------------------------------------- |
| unset (default)       | PTY when both stdin & stdout are TTYs, else exec         |
| `pty`                 | force PTY mode (ok for non-tty if you accept side effects) |
| `exec`                | force exec mode (no VT100 forwarding, lowest overhead)   |
| `none`                | currently behaves like exec; reserved for future use     |

PTY mode mirrors the child's output to your terminal AND to
`/tmp/vibe-bridge-screen.out` (active window) via `CMD_VT100_STREAM`. Window
size is synced on startup and on SIGWINCH.

Manual smoke-test script (run in a real terminal): `scripts/smoke_pty_real_codex.sh`.
