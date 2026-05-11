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

from .bootstrap import can_connect, ensure_daemon_running
from .forwarder import Forwarder
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
LEGACY_REAL_SOCK_PATH = "/tmp/vibe-real.sock"
DEFAULT_LCD_COLS = 78
DEFAULT_LCD_ROWS = 15
TABLE_SEP_CELL_RE = re.compile(r"^:?-{3,}:?$")

LCD_CHAR_REPLACEMENTS = str.maketrans(
    {
        "⏺": "·",
        "⎿": "`",
        "╭": "+",
        "╮": "+",
        "╰": "+",
        "╯": "+",
        "┌": "+",
        "┐": "+",
        "└": "+",
        "┘": "+",
        "├": "+",
        "┤": "+",
        "┬": "+",
        "┴": "+",
        "┼": "+",
        "─": "-",
        "━": "-",
        "═": "-",
        "│": "|",
        "┃": "|",
        "║": "|",
        "·": "·",
        "∙": "·",
        "•": "·",
        "◦": "·",
        "▪": "*",
        "▫": "*",
        "■": "*",
        "□": "*",
        "◆": "*",
        "◇": "*",
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


class LcdOutputAdapter:
    """Small, stateful output adapter for the SG2002 LCD terminal."""

    def __init__(self) -> None:
        self._decoder = codecs.getincrementaldecoder("utf-8")("surrogateescape")
        self._pending_table_header: Optional[Tuple[List[str], str]] = None
        self._table_rows: Optional[List[List[str]]] = None

    def feed(self, data: bytes) -> bytes:
        text = self._decoder.decode(data, final=False)
        if not text:
            return b""
        text = text.translate(LCD_CHAR_REPLACEMENTS)
        return self._render_live_markdown(text).encode("utf-8", errors="surrogateescape")

    def _render_live_markdown(self, text: str) -> str:
        out: List[str] = []
        for line in text.splitlines(keepends=True):
            if not line.endswith(("\n", "\r")):
                out.append(self._flush_table_state())
                out.append(line)
                continue
            out.append(self._process_complete_line(line))
        return "".join(out)

    def _process_complete_line(self, line: str) -> str:
        if "\x1b" in line:
            return self._flush_table_state() + line

        row = _parse_markdown_table_row(line)
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


def _strip_line_ending(line: str) -> Tuple[str, str]:
    if line.endswith("\r\n"):
        return line[:-2], "\r\n"
    if line.endswith("\n") or line.endswith("\r"):
        return line[:-1], line[-1]
    return line, ""


def _parse_markdown_table_row(line: str) -> Optional[List[str]]:
    body, _ = _strip_line_ending(line)
    body = body.strip()
    if "|" not in body:
        return None
    if body.startswith("|"):
        body = body[1:]
    if body.endswith("|"):
        body = body[:-1]
    cells = [cell.strip() for cell in body.split("|")]
    return cells if len(cells) >= 2 else None


def _is_markdown_table_separator(line: str) -> bool:
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
    border = "+" + "+".join("-" * (width + 2) for width in widths) + "+"
    rendered: List[str] = [border]
    for row_idx, row in enumerate(padded):
        rendered.append(
            "|"
            + "|".join(f" {_pad_lcd_text(cell, widths[col])} " for col, cell in enumerate(row))
            + "|"
        )
        if row_idx == 0:
            rendered.append(border)
    rendered.append(border)
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
        return env_sock
    if can_connect(DEFAULT_SOCK_PATH):
        return DEFAULT_SOCK_PATH
    if can_connect(LEGACY_REAL_SOCK_PATH):
        return LEGACY_REAL_SOCK_PATH
    return DEFAULT_SOCK_PATH


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
    adapter = None if os.environ.get(ENV_LCD_CHAR_ADAPT) == "0" else LcdOutputAdapter()

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
            return plugin, existing
        except OSError:
            return None, None

    plugin: Optional[PluginClient] = None
    try:
        plugin = PluginClient(plugin_name=plugin_name, sock_path=sock_path)
        plugin.connect()
        sid = plugin.acquire_session(timeout=2.0)
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
