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

## Windows product mode

The formal product path is a native Windows host, not WSL hidraw mounting.  The
board protocol stays unchanged: board-assigned `session_id`, HID packets,
VT100 stream chunks, `WINDOW_ACTIVATE`, key events, and encoder events remain
the contract between host and board.

Windows cannot reliably "hook every agent window" as one generic desktop
capture problem.  The product splits that into explicit session adapters:

- `windows-cli`: launch Windows CLI tools under a native shim/ConPTY and create
  one bridge session per top-level tool window.
- `wsl-cli`: launch `wsl.exe` from the Windows host shim and capture the owning
  Windows terminal session, avoiding a WSL-installed wrapper as the product
  control plane.
- `vscode`: a companion VS Code extension registers extension-host terminals,
  webviews, and agent panels with the local bridge.
- `browser`: a browser extension registers supported agent tabs through Native
  Messaging or local IPC; the desktop host can focus the window, but the
  extension owns tab/page semantics.

Use the Windows readiness commands before building the native host:

```powershell
python -m vibe_bridge.main windows plan
python -m vibe_bridge.main windows doctor
```

Run the Windows host as a single daemon.  This process owns the USB HID handle;
all CLI/WSL/VS Code/browser adapters connect to its local IPC and must not open
the board directly:

```powershell
# Normal hardware path: auto-select Vibe HID 359f:2120 and listen on localhost.
python -m vibe_bridge.main windows daemon

# IPC-only development path, useful before a board is attached.
python -m vibe_bridge.main windows daemon --device none
```

Adapters use the same packet framing over loopback TCP:

```powershell
$env:VIBE_SOCK_PATH = "tcp://127.0.0.1:8765"
```

Run a CLI through the first Windows adapter:

```powershell
# In a second PowerShell while `windows daemon` is still running:
python -m vibe_bridge.main windows cli -- codex

# WSL command-line tools must be launched from the Windows host side:
python -m vibe_bridge.main windows wsl-cli -- codex
```

Presentation model:

- One background daemon is the product service/tray-process candidate.
- `windows-cli` and `wsl-cli` are launcher shims; each top-level agent run
  requests a board session id, forwards VT100 through the daemon, and records
  enough foreground-window metadata for later activation.
- VS Code and browser support require companion extensions.  The desktop daemon
  can activate top-level windows, but editor panels, webviews, and browser tabs
  must be registered by their owning extension.

`windows doctor` reports `NOT READY` until it is running on native Windows with
a reachable Vibe HID device and at least one adapter implemented.  This prevents
the old WSL/hidraw wrapper path from being mistaken for the Windows product.

## Real hardware deployment (Linux/WSL development path)

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

Probe the board. The script auto-selects the `359f:2120` hidraw node; pass an
explicit path only when you intentionally override it:

```bash
PYTHONPATH=src python3 -m vibe_bridge.main hid list
./scripts/probe_hidraw.sh
```

A healthy new-protocol board prints `RESULT=PASS` and the LCD should show the
probe text. If the device exists but is not writable, fix hidraw permissions
with a udev rule or a temporary local permission change for that node.

### Automated install (recommended)

Run the one-shot installer once per host. It installs the `codex` / `claude`
wrapper symlinks under `~/.local/bin`, backs the real CLIs up to
`~/.local/share/vibe-bridge/real-bin/`, and appends a PATH block to
`~/.bashrc` (or `~/.zshrc` if `$SHELL` is zsh):

```bash
bash ./install_host.sh
# then start a new shell, or:
source ~/.bashrc
```

Check or uninstall the host wrapper setup without guessing what changed:

```bash
bash ./install_host.sh --check
bash ./install_host.sh --uninstall
```

After this, `command -v codex` resolves to the wrapper and you do not need to
run any daemon yourself; the wrapper spawns a detached daemon on first use (see
"Normal use" below).

The manual symlink path is still supported if you prefer not to touch your shell
rc file:

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

### Normal use

After install:

```bash
codex
```

The wrapper auto-starts the daemon if needed (via
`bootstrap.ensure_daemon_running`, which `Popen`s a detached process with its
own session — the daemon survives wrapper exit and is shared across multiple
`codex` / `claude` runs). It also auto-detects the Vibe HID device by VID/PID
`359f:2120`, requests a fresh board-assigned session id, and mirrors the
interactive PTY output to the LCD. Opening another top-level `codex` creates
another window.

The foreground `python3 -m vibe_bridge.main daemon` invocation shown in
"Quick start" above is only for mock-mode demos and `scripts/smoke_*.sh`; you
do not need to keep it running for normal `codex` / `claude` use.

Press `SESSION` to enter session selection, rotate the encoder to replay
existing windows, then press `CONFIRM` or the encoder switch to keep the
selected window. Exiting a wrapper releases its window; if it was active, the
daemon switches to another live window and replays that buffer.

When a real HID board attaches, the daemon also pushes a small connection
status panel to the dashboard: bridge state, attached device, and live session
count. Product keys that do not yet have full tool adapters still have visible
closed-loop panels on the board:

- `VOICE`: voice standby, planned push-to-talk behaviour, and explicit “no text injection yet”.
- `AGENT_MODEL`: model/preset selection placeholder.
- `VOTE_REVIEW`: approve/revise/review action placeholder.
- `MULTI_FUNCTION`: utility shortcut placeholder.
- `MENU_DEBUG`: host/HID status panel. The current 7-key board no longer has
  a dedicated physical MENU key; bit7 is kept for protocol compatibility.

Press `REJECT`, `CONFIRM`, or the encoder switch to close these panels.

### Board controls

The board keeps the physical pin order stable and reports button state through
`CMD_KEY_EVENT`. The daemon routes key events to the active plugin/wrapper; it
does not hard-code product actions. Board-local video feedback uses the same
semantic names.

| Input | Pin | HID bit | Name | Behaviour |
| --- | --- | --- | --- | --- |
| KEY1 | P19 | bit0 | `REJECT` | Cancel, reject, stop the current AI task, or go back one layer. |
| KEY2 | A22 | bit1 | `VOICE` | Enter voice input; hold-to-record/release-to-send when recording is available, otherwise show the voice placeholder state. |
| KEY3 | A25 | bit2 | `SESSION` | Open the session or task selection panel. |
| KEY4 | A27 | bit3 | `VOTE_REVIEW` | Open rating, choice, or review controls for the current AI output. |
| KEY5 | A23 | bit4 | `AGENT_MODEL` | Open model/agent selection for Claude, Codex, Gemini, Local, or fast/deep modes. |
| KEY6 | A24 | bit5 | `MULTI_FUNCTION` | Open low-frequency shortcuts: save, commit, apply all, screenshot, settings, brightness, volume, and network state. |
| KEY7 | A15 | bit6 | `CONFIRM` | Confirm, send, execute the highlighted action, or apply the current suggestion. |
| - | - | bit7 | `MENU_DEBUG` | Protocol-compatible reserved bit; no physical key on the current board. |
| ENC_SW | P21 | payload[1] bit0 | `SELECT_ENTER` | Light confirm, select, or drill into the highlighted item. |
| ENC_A / ENC_B | P22 / P23 | `ENCODER_EVENT` delta | `ROTARY` | Scroll lists, choose candidates, and adjust parameters. |

Use the encoder push for light selection and `CONFIRM` for stronger execute or
apply semantics.

When a `codex` or `claude` wrapper owns the active session, only low-level
navigation keys are translated into PTY actions: `REJECT` sends Ctrl-C,
`CONFIRM` / `ENC_SW` sends Enter, and encoder deltas send Up/Down arrows.
`VOICE`, `VOTE_REVIEW`, `AGENT_MODEL`, and `MULTI_FUNCTION` are
board-local video feedback for now; they do not inject synthetic text into the
active tool. Set `VIBE_BRIDGE_BOARD_ACTIONS=0` to disable all wrapper PTY
injection while keeping normal LCD mirroring.

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

For product-style diagnosis, prefer `doctor`; it checks daemon reachability,
state freshness, HID VID/PID, wrapper PATH, and whether the real CLI is still
reachable behind the wrapper:

```bash
PYTHONPATH=/path/to/vibe-bridge/src python3 -m vibe_bridge.main doctor
PYTHONPATH=/path/to/vibe-bridge/src python3 -m vibe_bridge.main doctor --color never
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
vibe-bridge ensure-daemon                       # detached daemon + socket/log status
vibe-bridge doctor                              # host install + daemon + HID health check
vibe-bridge sessions                            # print session table
vibe-bridge request-session [--plugin NAME]     # smoke handshake test
vibe-bridge send-vt100 --sid N --text "..."     # send a VT100 chunk
vibe-bridge tail-screen                         # dump the screen file
vibe-bridge window-switch --delta {-1,0,1}      # rotate active window
vibe-bridge window-activate --sid N             # activate a specific session
vibe-bridge windows plan                        # Windows adapter/product plan
vibe-bridge windows doctor                      # Windows readiness check
vibe-bridge windows daemon                      # Windows daemon on tcp://127.0.0.1:8765
vibe-bridge windows cli -- codex                # run CLI under Windows ConPTY
vibe-bridge windows wsl-cli -- codex            # run WSL CLI from the Windows host
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
Automatic discovery only attaches to the Vibe board VID:PID (`359f:2120`);
it will not grab an arbitrary keyboard or other single `/dev/hidraw*` node.
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
- [x] Daemon/wrapper handlers for `CMD_KEY_EVENT` / `CMD_ENCODER_EVENT` (session selector plus active-tool PTY actions)
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
