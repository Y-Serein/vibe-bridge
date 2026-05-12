"""Shell-wrapper entry point.

Used by ``bin/codex``, ``bin/claude`` and friends. The wrapper:

1. Locates the real CLI binary by walking ``$PATH`` and skipping itself.
2. Ensures the vibe-bridge daemon is running (spawns it detached if not).
3. Acquires a session id and stamps it into the environment as
   ``VIBE_SESSION_ID`` and ``VIBE_SOCK_PATH`` for the child to inherit.
4. Either:
   - **PTY mode** (default when stdin & stdout are TTYs): keeps the wrapper
     alive as a parent that owns the PTY master; tees the child's output to
     the user's terminal AND to a forwarder that publishes
     ``CMD_VT100_STREAM`` packets to the daemon.
   - **Exec mode** (non-tty, or ``VIBE_BRIDGE_FORWARD=exec``): ``execvp`` into
     the real CLI for zero overhead. No VT100 forwarding.

If the daemon is unreachable or the session handshake fails, the wrapper logs a
warning and still runs the real CLI so the user is never blocked by the bridge
being broken.
"""

from __future__ import annotations

import codecs
import os
import re
import sys
from typing import List, Optional, Tuple

from .bootstrap import (
    DEFAULT_STATE_PATH,
    _load_daemon_state,
    can_connect,
    ensure_daemon_running,
    resolve_hidraw_device,
)
from .forwarder import Forwarder
from .hid_protocol import Cmd, Packet, ReportId
from .mock_hid import DEFAULT_SOCK_PATH
from .plugin_client import PluginClient, PluginError

ENV_SESSION_ID = "VIBE_SESSION_ID"
ENV_SOCK_PATH = "VIBE_SOCK_PATH"
ENV_DISABLE = "VIBE_BRIDGE_DISABLE"
ENV_FORWARD_MODE = "VIBE_BRIDGE_FORWARD"  # "pty" | "exec" | unset (=auto)
ENV_REUSE_SESSION = "VIBE_BRIDGE_REUSE_SESSION"
ENV_LCD_COLS = "VIBE_BRIDGE_LCD_COLS"
ENV_LCD_ROWS = "VIBE_BRIDGE_LCD_ROWS"
ENV_LCD_CHAR_ADAPT = "VIBE_BRIDGE_LCD_CHAR_ADAPT"
ENV_LCD_THEME = "VIBE_BRIDGE_LCD_THEME"
LEGACY_REAL_SOCK_PATH = "/tmp/vibe-real.sock"
LEGACY_REAL_STATE_PATH = "/tmp/vibe-real-state.json"
DEFAULT_LCD_COLS = 78
DEFAULT_LCD_ROWS = 15
TABLE_SEP_CELL_RE = re.compile(r"^:?-{3,}:?$")
SGR_RE = re.compile(r"\x1b\[([0-9;:]*)m")
ANSI_RE = re.compile(r"\x1b\[[?0-9;:]*[@-~]|\x1b\].*?(?:\x07|\x1b\\)|\x1b[_PX^].*?\x1b\\", re.DOTALL)

LCD_CHAR_REPLACEMENTS = str.maketrans(
    {
        "⏺": "·",
        "⎿": "`",
        "·": "·",
        "∙": "·",
        "•": "·",
        "◦": "·",
        "▪": "·",
        "▫": "·",
        "■": "·",
        "□": "·",
        "◆": "·",
        "◇": "·",
        "●": "·",
        "○": "o",
        "✓": "v",
        "✔": "v",
        "✗": "x",
        "✘": "x",
        "×": "x",
        "…": "...",
        "→": "->",
        "←": "<-",
        "›": ">",
        "❯": ">",
        "❱": ">",
        "❭": ">",
        "⟩": ">",
        "▸": ">",
        "▹": ">",
        "▶": ">",
        "▻": ">",
        "‹": "<",
        "❮": "<",
        "❰": "<",
        "❬": "<",
        "⟨": "<",
        "◂": "<",
        "◃": "<",
        "◀": "<",
        "◅": "<",
        "“": '"',
        "”": '"',
        "‘": "'",
        "’": "'",
    }
)

# RGB values come from docs/color.table / design/palette/gruvbox.color.table.
GRUVBOX_DARK_BG = (40, 40, 40)  # dark0
GRUVBOX_SOFT_BG = (60, 56, 54)  # dark1
GRUVBOX_DEFAULT_FG = (235, 219, 178)  # light1
GRUVBOX_PALETTE = (
    (29, 32, 33),  # dark0_hard
    (40, 40, 40),  # dark0
    (50, 48, 47),  # dark0_soft
    (60, 56, 54),  # dark1
    (80, 73, 69),  # dark2
    (102, 92, 84),  # dark3
    (124, 111, 100),  # dark4
    (146, 131, 116),  # gray_245
    (249, 245, 215),  # light0_hard
    (251, 241, 199),  # light0
    (242, 229, 188),  # light0_soft
    (235, 219, 178),  # light1
    (213, 196, 161),  # light2
    (189, 174, 147),  # light3
    (168, 153, 132),  # light4
    (251, 73, 52),  # bright_red
    (184, 187, 38),  # bright_green
    (250, 189, 47),  # bright_yellow
    (131, 165, 152),  # bright_blue
    (211, 134, 155),  # bright_purple
    (142, 192, 124),  # bright_aqua
    (254, 128, 25),  # bright_orange
    (204, 36, 29),  # neutral_red
    (152, 151, 26),  # neutral_green
    (215, 153, 33),  # neutral_yellow
    (69, 133, 136),  # neutral_blue
    (177, 98, 134),  # neutral_purple
    (104, 157, 106),  # neutral_aqua
    (214, 93, 14),  # neutral_orange
    (157, 0, 6),  # faded_red
    (121, 116, 14),  # faded_green
    (181, 118, 20),  # faded_yellow
    (7, 102, 120),  # faded_blue
    (143, 63, 113),  # faded_purple
    (66, 123, 88),  # faded_aqua
    (175, 58, 3),  # faded_orange
)
GRUVBOX_ANSI = {
    0: (40, 40, 40),  # dark0
    1: (204, 36, 29),  # neutral_red
    2: (152, 151, 26),  # neutral_green
    3: (215, 153, 33),  # neutral_yellow
    4: (69, 133, 136),  # neutral_blue
    5: (177, 98, 134),  # neutral_purple
    6: (104, 157, 106),  # neutral_aqua
    7: (168, 153, 132),  # light4
    8: (146, 131, 116),  # gray_245
    9: (251, 73, 52),  # bright_red
    10: (184, 187, 38),  # bright_green
    11: (250, 189, 47),  # bright_yellow
    12: (131, 165, 152),  # bright_blue
    13: (211, 134, 155),  # bright_purple
    14: (142, 192, 124),  # bright_aqua
    15: (235, 219, 178),  # light1
}


class LcdOutputAdapter:
    """Small, stateful output adapter for the SG2002 LCD terminal."""

    def __init__(self, *, theme: Optional[str] = None) -> None:
        self._decoder = codecs.getincrementaldecoder("utf-8")("surrogateescape")
        self._pending_table_header: Optional[Tuple[List[str], str]] = None
        self._table_rows: Optional[List[List[str]]] = None
        self._theme = (theme or "").strip().lower()
        self._theme_started = False
        self._theme_tail = ""
        self._compat_tail = ""
        self._partial_line = ""

    def feed(self, data: bytes) -> bytes:
        text = self._decoder.decode(data, final=False)
        if not text:
            return b""
        text = text.translate(LCD_CHAR_REPLACEMENTS)
        text = self._render_live_markdown(text)
        if self._theme == "gruvbox":
            text = self._apply_gruvbox_theme(text)
        else:
            text = self._apply_terminal_compat(text)
        return text.encode("utf-8", errors="surrogateescape")

    def _render_live_markdown(self, text: str) -> str:
        out: List[str] = []
        pending = self._partial_line + text
        self._partial_line = ""
        lines = pending.splitlines(keepends=True)
        if lines and not lines[-1].endswith(("\n", "\r")):
            tail = lines.pop()
        else:
            tail = ""

        for line in lines:
            out.append(self._process_complete_line(line))

        if tail:
            plain_tail = _strip_sgr(tail)
            if (
                self._table_rows is not None
                or self._pending_table_header is not None
                or "|" in plain_tail
            ):
                self._partial_line = tail
            else:
                out.append(tail)
        return "".join(out)

    def _process_complete_line(self, line: str) -> str:
        if _is_non_reply_line(line):
            return self._flush_table_state() + line

        plain_line = _strip_ansi(line)
        row = _parse_markdown_table_row(plain_line)
        if self._table_rows is not None:
            if row is not None:
                self._table_rows.append(row)
                return ""
            return self._flush_table_state() + self._process_complete_line(line)

        if self._pending_table_header is not None:
            header, original = self._pending_table_header
            self._pending_table_header = None
            if _is_markdown_table_separator(line):
                self._table_rows = [header]
                return ""
            return original + self._process_complete_line(line)

        if row is not None:
            self._pending_table_header = (row, line)
            return ""
        return line

    def _flush_table_state(self) -> str:
        out = ""
        if self._table_rows is not None:
            out += _render_lcd_table(self._table_rows)
            self._table_rows = None
        if self._pending_table_header is not None:
            out += self._pending_table_header[1]
            self._pending_table_header = None
        return out

    def _apply_gruvbox_theme(self, text: str) -> str:
        pending = self._theme_tail + text
        body, self._theme_tail = _split_incomplete_sgr_tail(pending)
        if not body:
            return ""
        body = SGR_RE.sub(_remap_sgr_to_gruvbox, body)
        if not self._theme_started:
            body = _rgb_sgr(38, GRUVBOX_DEFAULT_FG) + _rgb_sgr(48, GRUVBOX_DARK_BG) + body
            self._theme_started = True
        return body

    def _apply_terminal_compat(self, text: str) -> str:
        pending = self._compat_tail + text
        body, self._compat_tail = _split_incomplete_sgr_tail(pending)
        if not body:
            return ""
        return SGR_RE.sub(_remap_sgr_for_lcd_terminal, body)


def _strip_line_ending(line: str) -> Tuple[str, str]:
    if line.endswith("\r\n"):
        return line[:-2], "\r\n"
    if line.endswith("\n") or line.endswith("\r"):
        return line[:-1], line[-1]
    return line, ""


def _parse_markdown_table_row(line: str) -> Optional[List[str]]:
    body, _ = _strip_line_ending(line)
    body = body.strip()
    body = re.sub(r"^[>*›\-\s]*[·•*.]\s+(?=\|)", "", body)
    if "|" not in body:
        return None
    if body.startswith("|"):
        body = body[1:]
    if body.endswith("|"):
        body = body[:-1]
    cells = [cell.strip() for cell in body.split("|")]
    return cells if len(cells) >= 2 else None


def _is_non_reply_line(line: str) -> bool:
    if "\x1b[7m" in line:
        return True
    plain = _strip_ansi(line).strip()
    if not plain:
        return False
    if plain.startswith((">", "›", "❯", "❱")):
        return True
    if plain.startswith(("╭", "╰", "┌", "└", "│", "┃", "┏", "┗")):
        return True
    lower = plain.lower()
    return lower.startswith(
        (
            "openai codex",
            "model:",
            "directory:",
            "tip:",
            "gpt-",
            "claude",
            "aikb",
            "vt100",
        )
    )


def _is_markdown_table_separator(line: str) -> bool:
    body, _ = _strip_line_ending(_strip_ansi(line))
    body = body.strip()
    if body.count("|") >= 2 and not re.search(r"[^|\s:-]", body):
        return True
    cells = _parse_markdown_table_row(line)
    if not cells:
        return False
    return all(TABLE_SEP_CELL_RE.match(cell.replace(" ", "")) for cell in cells)


def _render_lcd_table(rows: List[List[str]]) -> str:
    if not rows:
        return ""
    col_count = max(len(row) for row in rows)
    padded = [row + [""] * (col_count - len(row)) for row in rows]
    widths = [max(_lcd_text_width(row[col]) for row in padded) for col in range(col_count)]
    rendered: List[str] = []
    for row in padded:
        rendered.append(
            "  ".join(_pad_lcd_text(cell, widths[col]) for col, cell in enumerate(row)).rstrip()
        )
        if len(rendered) == 1:
            rendered.append("")
    return "\n".join(rendered) + "\n"


def _pad_lcd_text(text: str, width: int) -> str:
    return text + " " * max(0, width - _lcd_text_width(text))


def _lcd_text_width(text: str) -> int:
    width = 0
    for ch in text:
        cp = ord(ch)
        if (
            0x1100 <= cp <= 0x115F
            or 0x2E80 <= cp <= 0xA4CF
            or 0xAC00 <= cp <= 0xD7A3
            or 0xF900 <= cp <= 0xFAFF
            or 0xFE10 <= cp <= 0xFE6F
            or 0xFF00 <= cp <= 0xFF60
            or 0xFFE0 <= cp <= 0xFFE6
            or 0x20000 <= cp <= 0x3FFFD
        ):
            width += 2
        else:
            width += 1
    return width


def _strip_sgr(text: str) -> str:
    return SGR_RE.sub("", text)


def _strip_ansi(text: str) -> str:
    return ANSI_RE.sub("", text)


def _split_incomplete_escape_tail(text: str) -> Tuple[str, str]:
    esc = text.rfind("\x1b")
    if esc < 0:
        return text, ""
    tail = text[esc:]
    if tail == "\x1b":
        return text[:esc], tail
    if tail.startswith("\x1b[") and not re.search(r"[\x40-\x7e]", tail[2:]):
        return text[:esc], tail
    if tail.startswith("\x1b]") and "\x07" not in tail and "\x1b\\" not in tail:
        return text[:esc], tail
    if tail.startswith(("\x1b_", "\x1bP", "\x1bX", "\x1b^")) and "\x1b\\" not in tail:
        return text[:esc], tail
    return text, ""


def _split_incomplete_sgr_tail(text: str) -> Tuple[str, str]:
    esc = text.rfind("\x1b")
    if esc < 0:
        return text, ""
    tail = text[esc:]
    if SGR_RE.fullmatch(tail):
        return text, ""
    if re.fullmatch(r"\x1b(?:\[?[0-9;:]*)?", tail):
        return text[:esc], tail
    return text, ""


def _rgb_sgr(kind: int, rgb: Tuple[int, int, int]) -> str:
    return f"\x1b[{kind};2;{rgb[0]};{rgb[1]};{rgb[2]}m"


def _theme_reset() -> str:
    return "\x1b[0m" + _rgb_sgr(38, GRUVBOX_DEFAULT_FG) + _rgb_sgr(48, GRUVBOX_DARK_BG)


def _remap_sgr_for_lcd_terminal(match: re.Match) -> str:
    params = _parse_sgr_params(match.group(1))
    out: List[str] = []
    i = 0
    while i < len(params):
        code = params[i]
        if code == 7:
            out.extend(_rgb_params(38, (249, 245, 215)))
            out.extend(_rgb_params(48, (80, 73, 69)))
        elif code == 27:
            out.extend(["39", "49"])
        else:
            out.append(str(code))
        i += 1
    return "\x1b[" + ";".join(out) + "m"


def _remap_sgr_to_gruvbox(match: re.Match) -> str:
    raw = match.group(1)
    if not raw:
        return "\x1b[0m" + _rgb_sgr(38, GRUVBOX_DEFAULT_FG) + _rgb_sgr(48, GRUVBOX_DARK_BG)
    params = _parse_sgr_params(raw)
    out: List[str] = []
    i = 0
    while i < len(params):
        code = params[i]
        if code == 0:
            out.append("0")
            out.extend(_rgb_params(38, GRUVBOX_DEFAULT_FG))
            out.extend(_rgb_params(48, GRUVBOX_DARK_BG))
        elif code == 39:
            out.extend(_rgb_params(38, GRUVBOX_DEFAULT_FG))
        elif code == 49:
            out.extend(_rgb_params(48, GRUVBOX_DARK_BG))
        elif code in {7, 27}:
            pass
        elif 30 <= code <= 37:
            out.extend(_rgb_params(38, GRUVBOX_ANSI[code - 30]))
        elif 90 <= code <= 97:
            out.extend(_rgb_params(38, GRUVBOX_ANSI[code - 90 + 8]))
        elif 40 <= code <= 47:
            out.extend(_rgb_params(48, GRUVBOX_SOFT_BG))
        elif 100 <= code <= 107:
            out.extend(_rgb_params(48, GRUVBOX_SOFT_BG))
        elif code in {38, 48} and i + 4 < len(params) and params[i + 1] == 2:
            rgb = _nearest_gruvbox((params[i + 2], params[i + 3], params[i + 4]))
            out.extend(_rgb_params(code, rgb if code == 38 else _nearest_gruvbox_bg(rgb)))
            i += 4
        elif code in {38, 48} and i + 2 < len(params) and params[i + 1] == 5:
            idx = params[i + 2]
            if 0 <= idx <= 15:
                rgb = GRUVBOX_ANSI[idx]
                out.extend(_rgb_params(code, rgb if code == 38 else _nearest_gruvbox_bg(rgb)))
            else:
                rgb = _nearest_gruvbox(_xterm_256_rgb(idx))
                out.extend(_rgb_params(code, rgb if code == 38 else _nearest_gruvbox_bg(rgb)))
            i += 2
        else:
            out.append(str(code))
        i += 1
    return "\x1b[" + ";".join(out) + "m" if out else ""


def _parse_sgr_params(raw: str) -> List[int]:
    params: List[int] = []
    for part in raw.replace(":", ";").split(";"):
        if not part:
            params.append(0)
            continue
        try:
            params.append(int(part))
        except ValueError:
            params.append(0)
    return params or [0]


def _rgb_params(kind: int, rgb: Tuple[int, int, int]) -> List[str]:
    return [str(kind), "2", str(rgb[0]), str(rgb[1]), str(rgb[2])]


def _nearest_gruvbox(rgb: Tuple[int, int, int]) -> Tuple[int, int, int]:
    r, g, b = rgb
    return min(
        GRUVBOX_PALETTE,
        key=lambda c: (c[0] - r) ** 2 + (c[1] - g) ** 2 + (c[2] - b) ** 2,
    )


def _nearest_gruvbox_bg(rgb: Tuple[int, int, int]) -> Tuple[int, int, int]:
    return GRUVBOX_SOFT_BG


def _xterm_256_rgb(idx: int) -> Tuple[int, int, int]:
    idx = max(0, min(255, idx))
    if idx < 16:
        return GRUVBOX_ANSI[idx]
    if 16 <= idx <= 231:
        n = idx - 16
        levels = (0, 95, 135, 175, 215, 255)
        return (levels[n // 36], levels[(n // 6) % 6], levels[n % 6])
    gray = 8 + (idx - 232) * 10
    return (gray, gray, gray)


def find_real_binary(name: str, *, exclude_paths: List[str]) -> Optional[str]:
    """Return the first ``name`` on PATH whose realpath is not in ``exclude_paths``."""
    excluded = {os.path.realpath(p) for p in exclude_paths}
    seen = set()
    for raw in os.environ.get("PATH", "").split(os.pathsep):
        d = os.path.expanduser(raw) if raw else "."
        if not d or d in seen:
            continue
        seen.add(d)
        candidate = os.path.join(d, name)
        if not os.access(candidate, os.X_OK):
            continue
        if not os.path.isfile(candidate):
            continue
        if os.path.realpath(candidate) in excluded:
            continue
        return candidate
    return None


def _select_mode() -> str:
    explicit = os.environ.get(ENV_FORWARD_MODE, "").strip().lower()
    if explicit in {"pty", "exec", "none"}:
        return explicit
    if sys.stdin.isatty() and sys.stdout.isatty():
        return "pty"
    return "exec"


def _resolve_sock_path(sock_path: Optional[str]) -> str:
    if sock_path:
        return sock_path
    env_sock = os.environ.get(ENV_SOCK_PATH)
    if env_sock:
        if _legacy_real_sock_should_fall_back(env_sock):
            return DEFAULT_SOCK_PATH
        return env_sock
    if can_connect(DEFAULT_SOCK_PATH):
        return DEFAULT_SOCK_PATH
    if can_connect(LEGACY_REAL_SOCK_PATH):
        return LEGACY_REAL_SOCK_PATH
    return DEFAULT_SOCK_PATH


def _legacy_real_sock_should_fall_back(env_sock: str) -> bool:
    if os.path.realpath(env_sock) != os.path.realpath(LEGACY_REAL_SOCK_PATH):
        return False
    if not can_connect(env_sock):
        return True

    legacy_state = _load_daemon_state(LEGACY_REAL_STATE_PATH)
    if legacy_state and legacy_state.get("mode") == "real-hidraw":
        return False

    default_state = _load_daemon_state(DEFAULT_STATE_PATH)
    if (
        can_connect(DEFAULT_SOCK_PATH)
        and default_state
        and default_state.get("mode") == "real-hidraw"
        and default_state.get("hidraw_path")
    ):
        return True

    return resolve_hidraw_device() is not None


def _existing_session_from_env() -> Optional[int]:
    if os.environ.get(ENV_REUSE_SESSION) != "1":
        return None
    existing = os.environ.get(ENV_SESSION_ID)
    if existing is None:
        return None
    try:
        return int(existing)
    except ValueError:
        return None


def _parse_positive_env_int(name: str, default: int) -> int:
    raw = os.environ.get(name)
    if raw is None:
        return default
    try:
        value = int(raw)
    except ValueError:
        return default
    return value if value > 0 else default


def _lcd_pty_size() -> Tuple[int, int]:
    rows = _parse_positive_env_int(ENV_LCD_ROWS, DEFAULT_LCD_ROWS)
    cols = _parse_positive_env_int(ENV_LCD_COLS, DEFAULT_LCD_COLS)
    return rows, cols


def _lcd_output_adapter_from_env() -> Optional[LcdOutputAdapter]:
    """Return the LCD adapter unless raw forwarding is explicitly requested."""
    enabled = os.environ.get(ENV_LCD_CHAR_ADAPT, "").strip().lower()
    if enabled in {"0", "false", "no", "off", "raw"}:
        return None
    theme = os.environ.get(ENV_LCD_THEME, "").strip().lower()
    if theme in {"0", "none", "off", "false"}:
        theme = ""
    return LcdOutputAdapter(theme=theme)


def run(
    plugin_name: str,
    *,
    argv: Optional[List[str]] = None,
    sock_path: Optional[str] = None,
    self_paths: Optional[List[str]] = None,
) -> int:
    argv = list(argv if argv is not None else sys.argv)
    sock_path = _resolve_sock_path(sock_path)
    self_paths = list(self_paths or [argv[0] if argv else __file__])

    if os.environ.get(ENV_DISABLE) == "1":
        return _exec_real(plugin_name, argv, self_paths)

    real = find_real_binary(plugin_name, exclude_paths=self_paths)
    if real is None:
        print(
            f"vibe-bridge wrapper: could not find a real `{plugin_name}` on PATH "
            f"(looked through $PATH skipping {self_paths})",
            file=sys.stderr,
        )
        return 127

    new_argv = [os.path.basename(real)] + argv[1:]
    mode = _select_mode()
    session_id: Optional[int] = None
    plugin: Optional[PluginClient] = None
    if mode == "pty":
        plugin, session_id = _open_session_client(plugin_name, sock_path)

    env = dict(os.environ)
    env[ENV_SOCK_PATH] = sock_path
    if session_id is not None:
        env[ENV_SESSION_ID] = str(session_id)
    if mode == "pty":
        rows, cols = _lcd_pty_size()
        env["LINES"] = str(rows)
        env["COLUMNS"] = str(cols)

    if mode == "pty" and session_id is not None and plugin is not None:
        return _run_pty(real, new_argv, env, plugin)
    if plugin is not None:
        plugin.close()
    return _run_exec(real, new_argv, env)


def _run_exec(real: str, new_argv: List[str], env: dict) -> int:
    try:
        os.execvpe(real, new_argv, env)
    except OSError as exc:
        print(f"vibe-bridge wrapper: failed to exec {real}: {exc}", file=sys.stderr)
        return 126
    return 0  # unreachable


def _run_pty(
    real: str,
    new_argv: List[str],
    env: dict,
    plugin: PluginClient,
) -> int:
    """Run real CLI under a PTY; tee output to the daemon as CMD_VT100_STREAM."""
    # Imports are local so exec mode stays free of the PTY dependency chain.
    from .pty_runner import run_with_pty

    forwarder = Forwarder(plugin.send_vt100)
    forwarder.start()
    adapter = _lcd_output_adapter_from_env()

    def on_output(chunk: bytes) -> None:
        forwarder.push(adapter.feed(chunk) if adapter is not None else chunk)

    real_argv = [real] + new_argv[1:]  # argv[0] should be the program path
    try:
        return run_with_pty(
            real_argv,
            env=env,
            on_output=on_output,
            winsize=_lcd_pty_size(),
        )
    finally:
        forwarder.stop(timeout=0.5)
        plugin.close()


def _open_session_client(
    plugin_name: str, sock_path: str
) -> Tuple[Optional[PluginClient], Optional[int]]:
    if not ensure_daemon_running(sock_path, timeout=3.0):
        print(
            "vibe-bridge wrapper: daemon unreachable, running without session",
            file=sys.stderr,
        )
        return None, None

    existing = _existing_session_from_env()
    if existing is not None:
        try:
            plugin = PluginClient(plugin_name=plugin_name, sock_path=sock_path)
            plugin.adopt_session(existing)
            plugin.connect()
            _activate_session(plugin, existing)
            return plugin, existing
        except OSError:
            return None, None

    plugin: Optional[PluginClient] = None
    try:
        plugin = PluginClient(plugin_name=plugin_name, sock_path=sock_path)
        plugin.connect()
        sid = plugin.acquire_session(timeout=2.0)
        _activate_session(plugin, sid)
        return plugin, sid
    except (PluginError, OSError) as exc:
        if plugin is not None:
            plugin.close()
        print(
            f"vibe-bridge wrapper: session handshake failed ({exc}); "
            "running without session",
            file=sys.stderr,
        )
        return None, None


def _activate_session(plugin: PluginClient, sid: int) -> None:
    try:
        plugin.send_packet(
            Packet(
                report_id=int(ReportId.HOST_BOUND),
                command=int(Cmd.WINDOW_ACTIVATE),
                session_id=sid,
                payload=b"",
            )
        )
    except Exception:
        pass


def _exec_real(plugin_name: str, argv: List[str], self_paths: List[str]) -> int:
    real = find_real_binary(plugin_name, exclude_paths=self_paths)
    if real is None:
        print(f"vibe-bridge wrapper: no real `{plugin_name}` on PATH", file=sys.stderr)
        return 127
    new_argv = [os.path.basename(real)] + argv[1:]
    try:
        os.execvp(real, new_argv)
    except OSError as exc:
        print(f"vibe-bridge wrapper: failed to exec {real}: {exc}", file=sys.stderr)
        return 126
    return 0  # unreachable
