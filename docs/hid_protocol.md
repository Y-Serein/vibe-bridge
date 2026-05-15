# HID packet protocol (v0.0.1)

This document is the source of truth for the on-the-wire format. Both the mock
transport (Unix socket) and the real HID transport (`/dev/hidraw0`) carry
exactly the same packet bytes; only the framing differs (real HID gives us
per-report framing for free, the mock transport prepends a 4-byte length).

## Packet header

```
offset  size  field            notes
------  ----  ---------------  -------------------------------------------
0       1     report_id        ReportId (HOST_BOUND=0x10, DEVICE_BOUND=0x20,
                                ACK=0x21, FEATURE=0x30)
1       1     command          Cmd enum (see below)
2       2     session_id       uint16 little-endian, 0 = broadcast/unset
4       2     payload_length   uint16 little-endian, big endian on wire is NOT used
6       N     payload          raw bytes
```

Header is fixed at **6 bytes**. Total packet ≤ `6 + payload_length`.

### HID frame size

A real HID report is capped at **64 bytes** (`MAX_HID_REPORT_SIZE`), so each
packet on a real `/dev/hidraw0` carries at most **58 bytes** of payload
(`MAX_PAYLOAD_PER_FRAME = 64 - 6`). Larger payloads (typically `CMD_VT100_STREAM`)
are fragmented by the sender:

```
fragment_payload(b"...") -> [chunk_0, chunk_1, ...]
stream_iter_packets(sid, b"...")  # yields ready-to-send Packets
```

The mock socket transport accepts arbitrary sizes but the helpers always
fragment so behaviour is identical on real HID.

## Commands (`command` byte)

| Hex   | Name                | Direction          | Payload semantics                              |
| ----- | ------------------- | ------------------ | ---------------------------------------------- |
| 0x01  | `REQUEST_SESSION`   | plugin → bridge    | optional UTF-8 plugin/wrapper hint             |
| 0x02  | `SESSION_RESPONSE`  | bridge → plugin    | `[Status]` (1 byte)                            |
| 0x03  | `SESSION_INVALID`   | bridge → plugin    | `[Status]`                                     |
| 0x10  | `KEY_EVENT`         | board → bridge → plugin | `[key_bits:u8, enc_sw:u8]`                 |
| 0x11  | `ENCODER_EVENT`     | board → bridge → plugin | `[delta:i8]`                               |
| 0x20  | `WINDOW_SWITCH`     | plugin/HW → bridge | `[delta:i8]` (-1 prev, +1 next, 0 noop)        |
| 0x21  | `WINDOW_ACTIVATE`   | plugin → bridge    | empty; activates `session_id`                  |
| 0x30  | `VT100_STREAM`      | plugin → bridge    | UTF-8 / VT100 byte stream                      |
| 0x40  | `UI_SCALE_CHANGE`   | plugin → bridge    | TBD: JSON `{font, line_height, col_width}`     |
| 0x50  | `STATUS_UPDATE`     | plugin → bridge    | UTF-8 JSON merged into session.context         |
| 0x60  | `FEEDBACK_EVENT`    | bridge → plugin    | TBD: `[type, intensity]` for haptics/LED/audio |
| 0xFF  | `ERROR`             | bridge → plugin    | UTF-8 message                                  |

`Status` byte values:

| Hex  | Name        | Meaning                                            |
| ---- | ----------- | -------------------------------------------------- |
| 0x00 | `OK`        | generic ack                                        |
| 0x01 | `CREATED`   | new session allocated (carries the new sid)        |
| 0x02 | `INVALID`   | sid does not exist (or never did)                  |
| 0x03 | `EXPIRED`   | sid was reaped after exceeding TTL                 |
| 0x04 | `POOL_FULL` | session pool exhausted, no eviction candidate      |
| 0x05 | `RECLAIMED` | sid was evicted (LRU) to make room for a new owner |

## Session id

- `session_id` is **always** carried in the header — even on
  `CMD_REQUEST_SESSION` where the field is set to `0` (broadcast / unset).
- Once `CMD_SESSION_RESPONSE` arrives, the plugin pins that sid for all
  subsequent packets. Sending with sid 0 after acquisition is invalid.
- The bridge **never trusts** an inbound sid: every non-handshake command goes
  through `_validate_session`, and unknown sids get a `CMD_SESSION_INVALID`
  with `Status.INVALID` reply.
- On `RECLAIMED` / `EXPIRED` the plugin must drop its stored sid and call
  `request_session` again. The default `PluginClient` does this automatically
  (`auto_reacquire=True`).

## Mock handshake example

```
plugin                                bridge daemon
  | --- REQUEST_SESSION (sid=0) ----->  |
  |                                     | session_manager.request_session(...)
  | <-- SESSION_RESPONSE (sid=N) -----  |
  |                                     | router.register(N)
  | --- VT100_STREAM    (sid=N) ----->  |
  |                                     | router.append(N, ...)
  | --- VT100_STREAM    (sid=N) ----->  |
  | --- WINDOW_ACTIVATE (sid=N) ----->  |
  |                                     | router.set_active(N)
                  ...
  | <-- SESSION_INVALID (RECLAIMED) --  |   (after pool full + LRU eviction)
  | --- REQUEST_SESSION (sid=0) ----->  |   (auto-reacquire)
  | <-- SESSION_RESPONSE (sid=M) -----  |
```

## Real-HID handshake example

In `daemon --hidraw /dev/hidraw0` mode, daemon startup only opens/probes the
hidraw node and drains stale input. It does not send `CMD_REQUEST_SESSION`.
Every plugin/wrapper session request is forwarded to the board, and the
board-returned `session_id` is treated as authoritative:

```
plugin/wrapper                 bridge daemon                    board firmware
  | --- REQUEST_SESSION ------>  | --- REQUEST_SESSION ---------> |
  |                              |                                | alloc_session(...)
  | <--- SESSION_RESPONSE ------ | <--- SESSION_RESPONSE(sid=N)-- |
  |                              | router.register(N)
  | --- VT100_STREAM(sid=N) ---> | --- VT100_STREAM(sid=N) -----> |
```

## Board input events

The early `docs/hid_vendor_terminal_spec.pdf` described a 7-key vendor-input
sketch where report `0x10` directly packed keys and encoder delta into report
bytes. The current implementation keeps the same report IDs but uses the
packet header above for session routing and command dispatch. Do not revert to
the PDF byte layout unless the whole board/host packet layer is intentionally
replaced.

Current AIKB firmware emits normal buttons and encoder push through
`CMD_KEY_EVENT`:

```
report_id        = 0x10  # HOST_BOUND, board -> host
command          = 0x10  # KEY_EVENT
session_id       = 0     # broadcast; daemon rewrites to active sid
payload_length   = 2
payload[0]       = key_bits
payload[1] bit0  = encoder push switch, pressed = 1
payload[1] bit1..7 reserved, send as 0
```

`payload[0]` key bit assignment:

| Bit | Physical input | Board action name | Current video state |
| --- | -------------- | ----------------- | ------------------- |
| 0   | KEY1 / A15     | Reject            | `reject.264`        |
| 1   | KEY2 / A24     | Voice             | `voice.264`         |
| 2   | KEY3 / A23     | Session           | `session.264`       |
| 3   | KEY4 / A27     | Vote / Review     | `vote_review.264`   |
| 4   | KEY5 / A25     | Agent / Model     | `agent_model.264`   |
| 5   | KEY6 / A22     | Multi Function    | `multi.264`         |
| 6   | KEY7 / A29     | Confirm           | `confirm.264`       |
| 7   | KEY8 / P19     | Menu / Debug      | `menu_debug.264`    |

Action intent:

- `REJECT`: cancel, reject, stop the current AI task, or go back one layer.
- `VOICE`: enter voice input. If recording is not implemented yet, show a
  voice-input placeholder state.
- `SESSION`: open the session/task selection panel.
- `VOTE_REVIEW`: open the rating, choice, or review panel for the current AI
  output.
- `AGENT_MODEL`: open the model/agent selection panel for Claude, Codex,
  Gemini, Local, or fast/deep mode switching.
- `MULTI_FUNCTION`: open a shortcut panel for lower-frequency actions such as
  save, commit, apply all, screenshot, settings, brightness, volume, and
  network status.
- `CONFIRM`: confirm, send, execute the highlighted action, or apply the
  current suggestion.
- `MENU_DEBUG`: short press opens the main menu or debug menu; reserved for
  future expansion.
- `ENC_SW / P21`: select/enter. This is a light confirm or drill-in action.

Encoder A/B (`P22` / `P23`) is used for list scrolling, candidate selection,
and parameter adjustment. Encoder push selects/enters; `CONFIRM` is reserved
for stronger execute/apply semantics.

The encoder quadrature delta is separate:

```
report_id        = 0x10  # HOST_BOUND, board -> host
command          = 0x11  # ENCODER_EVENT
session_id       = 0     # broadcast; daemon rewrites to active sid
payload_length   = 1
payload[0]       = signed int8 delta, -127..127
```

The bridge daemon handles encoder delta locally to switch active windows, then
routes the same `ENCODER_EVENT` to the active plugin. `KEY_EVENT` is only routed
to the active plugin; board-local video switching is handled by the board-side
event FIFO and `auto.sh`, not by the host daemon.

## Wire compatibility goals

- The on-board firmware (`middleware/v2/sample/aikb_hid_input/`) currently
  consumes a legacy `0x20` output report whose first payload byte is a
  sub-command (clear / write / cursor / newline / backspace). The new format
  *replaces* that scheme: legacy sub-commands map to `Cmd.VT100_STREAM` (write),
  and clear/cursor become VT100 escape sequences inside the payload.
- The board's vendor HID descriptor stays at `report_length=64`; the new header
  fits comfortably within the existing report size budget.
- `report_id=0x10` continues to be host-bound (board → host) for events; the
  difference is that events now also carry the active `session_id` so the
  daemon can route them to the right plugin.

## Open items

- **Fragmentation acks.** The new layer is fire-and-forget by default. A
  `CMD_VT100_STREAM` chunk ordering flag (e.g. payload prefix or feature report)
  is TBD if real hardware shows loss/reordering issues.
- **Feature reports** (`ReportId.FEATURE = 0x30`) are reserved for future
  config queries (font sizes, screen geometry).
- **Input action semantics beyond routing.** The transport now defines the
  board key bits and encoder bytes. Higher-level plugin behaviour for
  Reject/Voice/Session/Vote-Review/Agent-Model/Multi-Function/Confirm/Menu is
  still owned by the active plugin or board-side video state mapping; the
  daemon stays a packet router.
