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

## Real hardware deployment

Use this section when moving vibe-bridge to another Linux/WSL host.

Prerequisites:

- The board image contains the new-protocol `aikb_hid_input` and `aikb_lcd_ui`.
- The board is powered on and exposes a Vibe HID device with `VID:PID 359f:2120`.
- The host has Python >= 3.9 and the real CLI tools, such as `codex` or
  `claude`, already installed somewhere later on `$PATH`.

Install the host bridge:

```bash
git clone <repo-url> vibe-bridge
cd vibe-bridge
PYTHONPATH=src python3 -m unittest discover -s tests
PYTHONPATH=src python3 -m vibe_bridge.main hid list
```

Probe the board. Replace `/dev/hidraw0` if `hid list` shows a different node:

```bash
./scripts/probe_hidraw.sh /dev/hidraw0
```

A healthy new-protocol board prints `RESULT=PASS` and the LCD should show the
probe text. If the device exists but is not writable, fix hidraw permissions
with a udev rule or a temporary local permission change for that node.

Put the wrappers in front of the real CLIs. Symlinks are convenient when this
repo is not itself on `$PATH`:

```bash
mkdir -p ~/.local/bin
ln -sf "$PWD/bin/codex" ~/.local/bin/codex
ln -sf "$PWD/bin/claude" ~/.local/bin/claude
```

Make sure `~/.local/bin` appears before the real CLI install directory:

```bash
command -v codex
command -v claude
```

Normal use after setup is just:

```bash
codex
```

The wrapper auto-starts the daemon if needed, auto-detects the Vibe HID device,
requests a fresh board-assigned session id, and mirrors the interactive PTY
output to the LCD. Opening another top-level `codex` creates another window.
The encoder switches active windows. Exiting a wrapper releases its window; if
it was active, the daemon switches to another live window and replays that
buffer.

### Board controls

The board keeps the physical pin order stable and reports button state through
`CMD_KEY_EVENT`. The daemon routes key events to the active plugin/wrapper; it
does not hard-code product actions. Board-local video feedback uses the same
semantic names.

| Input | Pin | HID bit | Name | Behaviour |
| --- | --- | --- | --- | --- |
| KEY1 | A15 | bit0 | `REJECT` | Cancel, reject, stop the current AI task, or go back one layer. |
| KEY2 | A24 | bit1 | `VOICE` | Enter voice input; hold-to-record/release-to-send when recording is available, otherwise show the voice placeholder state. |
| KEY3 | A23 | bit2 | `SESSION` | Open the session or task selection panel. |
| KEY4 | A27 | bit3 | `VOTE_REVIEW` | Open rating, choice, or review controls for the current AI output. |
| KEY5 | A25 | bit4 | `AGENT_MODEL` | Open model/agent selection for Claude, Codex, Gemini, Local, or fast/deep modes. |
| KEY6 | A22 | bit5 | `MULTI_FUNCTION` | Open low-frequency shortcuts: save, commit, apply all, screenshot, settings, brightness, volume, and network state. |
| KEY7 | A29 | bit6 | `CONFIRM` | Confirm, send, execute the highlighted action, or apply the current suggestion. |
| KEY8 | P19 | bit7 | `MENU_DEBUG` | Open the main menu or debug menu; reserved for later expansion. |
| ENC_SW | P21 | payload[1] bit0 | `SELECT_ENTER` | Light confirm, select, or drill into the highlighted item. |
| ENC_A / ENC_B | P22 / P23 | `ENCODER_EVENT` delta | `ROTARY` | Scroll lists, choose candidates, and adjust parameters. |

Use the encoder push for light selection and `CONFIRM` for stronger execute or
apply semantics.

By default the LCD receives the PTY stream through a small compatibility layer:
missing prompt/bullet glyphs are downgraded to ASCII/`·`, reverse-video input
rows get a visible background, and Markdown table rendering is skipped for
prompt/system lines. Use raw forwarding only for diagnosis:

```bash
VIBE_BRIDGE_LCD_CHAR_ADAPT=0 codex
VIBE_BRIDGE_LCD_THEME=gruvbox codex
```

If `/tmp/vibe-bridge.sock` is reachable but its state file says `mode: mock`,
the wrapper does not reuse it when a Vibe HID device is present. It starts a
fresh `real-hidraw` daemon on the default socket so a stale mock daemon cannot
silently swallow Codex output.

Inspect the current daemon and session table:

```bash
PYTHONPATH=/path/to/vibe-bridge/src python3 -m vibe_bridge.main sessions
```

Useful overrides:

```bash
VIBE_HIDRAW_DEVICE=/dev/hidraw1 codex
VIBE_SOCK_PATH=/tmp/custom-vibe.sock codex
VIBE_BRIDGE_DISABLE=1 codex
VIBE_BRIDGE_LCD_CHAR_ADAPT=0 codex
```

After unplugging/replugging or rebooting the board, the hidraw node may change
and the board-side session table resets. If the old real daemon sees the
hidraw fd disappear, it marks its state as unavailable so the next wrapper run
can start a fresh real daemon. Use `sessions` to confirm the daemon is attached
to the current `/dev/hidraw*`.

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

108 tests, no external dependencies.

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
   there, or if the default socket is still a mock daemon while a Vibe HID
   device (`VID:PID 359f:2120`) is present.
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
