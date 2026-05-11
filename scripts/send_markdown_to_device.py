#!/usr/bin/env python3
"""Send Markdown/text to a real vibe-bridge HID stream with Kitty image adaptation."""

from __future__ import annotations

import argparse
import base64
import io
import math
import os
from pathlib import Path
import re
import sys
import time
from typing import Callable, List, Optional, Tuple
from urllib.parse import unquote, urlparse


KITTY_START = b"\x1b_G"
KITTY_END = b"\x1b\\"
TEXT_ENCODING = "utf-8"
MARKDOWN_IMAGE_RE = re.compile(r"!\[([^\]]*)\]\(([^)\r\n]+)\)")
TABLE_SEP_CELL_RE = re.compile(r"^:?-{3,}:?$")
MARKDOWN_HEADING_RE = re.compile(r"^(#{1,6})[ \t]+(.+?)[ \t#]*$")


class RenderError(Exception):
    pass


class ImageAdapter:
    def __init__(
        self,
        *,
        base_dir: Path,
        image_cols: int,
        max_image_rows: int,
        cell_w: int,
        cell_h: int,
        chunk_size: int,
        advance_cursor: bool,
    ) -> None:
        self.base_dir = base_dir
        self.image_cols = image_cols
        self.max_image_rows = max_image_rows
        self.cell_w = cell_w
        self.cell_h = cell_h
        self.chunk_size = chunk_size
        self.advance_cursor = advance_cursor
        self._next_image_id = 1

    def warning_bytes(self, message: str) -> bytes:
        print(f"warning: {message}", file=sys.stderr)
        return f"[image skipped: {message}]\r\n".encode(TEXT_ENCODING)

    def render_markdown_image(self, raw_target: str) -> bytes:
        target = parse_markdown_link_target(raw_target)
        local_path, warning = self.resolve_local_path(target)
        if warning is not None:
            return self.warning_bytes(warning)
        if local_path is None:
            return self.warning_bytes(f"unsupported image target: {target}")
        try:
            png, cols, rows = self.load_resize_as_png(local_path)
        except RenderError as exc:
            return self.warning_bytes(str(exc))

        image_id = self._next_image_id
        placement_id = image_id
        self._next_image_id += 1

        data = build_kitty_png_sequence(
            png,
            cols=cols,
            rows=rows,
            image_id=image_id,
            placement_id=placement_id,
            chunk_size=self.chunk_size,
        )
        if self.advance_cursor:
            data += b"\r\n" * max(1, rows)
        return data

    def resolve_local_path(self, target: str) -> Tuple[Optional[Path], Optional[str]]:
        parsed = urlparse(target)
        scheme = parsed.scheme.lower()
        if scheme in ("http", "https"):
            return None, f"remote URLs are not supported: {target}"
        if scheme == "file":
            if parsed.netloc not in ("", "localhost"):
                return None, f"unsupported file URL host: {target}"
            path = Path(unquote(parsed.path)).expanduser()
            return path, None
        if scheme and len(scheme) > 1:
            return None, f"unsupported URI scheme: {scheme}"

        path = Path(unquote(target)).expanduser()
        if not path.is_absolute():
            path = self.base_dir / path
        return path, None

    def load_resize_as_png(self, path: Path) -> Tuple[bytes, int, int]:
        if not path.exists():
            raise RenderError(f"local image not found: {path}")
        if not path.is_file():
            raise RenderError(f"image target is not a file: {path}")

        try:
            from PIL import Image
        except ImportError as exc:
            raise RenderError("Pillow is required for PNG/JPG/WebP conversion") from exc

        try:
            with Image.open(path) as img:
                img = img.convert("RGBA")
                src_w, src_h = img.size
                if src_w <= 0 or src_h <= 0:
                    raise RenderError(f"invalid image dimensions: {path}")

                cols = max(1, self.image_cols)
                max_rows = max(1, self.max_image_rows)
                target_w = max(1, cols * self.cell_w)
                aspect_h = max(1, round(src_h * target_w / src_w))
                rows = max(1, min(max_rows, math.ceil(aspect_h / self.cell_h)))
                target_h = max(1, rows * self.cell_h)

                resample = getattr(getattr(Image, "Resampling", Image), "LANCZOS")
                resized = img.copy()
                resized.thumbnail((target_w, target_h), resample)

                out = io.BytesIO()
                resized.save(out, format="PNG", optimize=True)
                return out.getvalue(), cols, rows
        except RenderError:
            raise
        except Exception as exc:
            raise RenderError(f"failed to convert image {path}: {exc}") from exc


def parse_markdown_link_target(raw: str) -> str:
    value = raw.strip()
    if value.startswith("<"):
        end = value.find(">")
        if end > 0:
            return value[1:end].strip()

    # First version only needs simple local paths. This also handles the common
    # optional-title form: ![alt](path "title").
    match = re.match(r"([^ \t]+)(?:[ \t]+['\"].*)?$", value)
    if match:
        return match.group(1)
    return value


def build_kitty_png_sequence(
    png: bytes,
    *,
    cols: int,
    rows: int,
    image_id: int,
    placement_id: int,
    chunk_size: int,
) -> bytes:
    encoded = base64.b64encode(png).decode("ascii")
    parts: List[bytes] = []
    for off in range(0, len(encoded), chunk_size):
        piece = encoded[off : off + chunk_size]
        more = 1 if off + chunk_size < len(encoded) else 0
        control = (
            f"a=T,f=100,t=d,q=2,c={cols},r={rows},"
            f"i={image_id},p={placement_id},m={more}"
        )
        parts.append(
            KITTY_START
            + control.encode("ascii")
            + b";"
            + piece.encode("ascii")
            + KITTY_END
        )
    return b"".join(parts)


def split_preserving_kitty_blocks(data: bytes) -> List[Tuple[bool, bytes]]:
    """Return (is_kitty_block, bytes) chunks without altering existing Kitty APC."""
    chunks: List[Tuple[bool, bytes]] = []
    pos = 0
    while pos < len(data):
        start = data.find(KITTY_START, pos)
        if start < 0:
            chunks.append((False, data[pos:]))
            break
        if start > pos:
            chunks.append((False, data[pos:start]))
        end = data.find(KITTY_END, start + len(KITTY_START))
        if end < 0:
            chunks.append((True, data[start:]))
            break
        end += len(KITTY_END)
        chunks.append((True, data[start:end]))
        pos = end
    return chunks


def render_text_chunk(chunk: bytes, render_image: Callable[[str], bytes]) -> bytes:
    text = chunk.decode(TEXT_ENCODING, errors="surrogateescape")
    out: List[bytes] = []
    in_fence = False
    lines = text.splitlines(keepends=True)
    idx = 0

    while idx < len(lines):
        line = lines[idx]
        body, eol = strip_line_ending(line)
        stripped = line.lstrip()
        if stripped.startswith("```") or stripped.startswith("~~~"):
            if in_fence:
                in_fence = False
            else:
                in_fence = True
                lang = stripped[3:].strip()
                label = f"Code: {lang}" if lang else "Code:"
                out.append((label + eol).encode(TEXT_ENCODING, errors="surrogateescape"))
            idx += 1
            continue
        if in_fence:
            out.append(("  " + line).encode(TEXT_ENCODING, errors="surrogateescape"))
            idx += 1
            continue
        heading = MARKDOWN_HEADING_RE.match(body.strip())
        if heading:
            out.append(f"== {heading.group(2)} =={eol}".encode(TEXT_ENCODING))
            idx += 1
            continue
        if idx + 1 < len(lines) and is_markdown_table_header(line, lines[idx + 1]):
            table_lines, next_idx = collect_markdown_table(lines, idx)
            out.append(render_markdown_table(table_lines).encode(TEXT_ENCODING))
            idx = next_idx
            continue
        image_match = MARKDOWN_IMAGE_RE.fullmatch(body.strip())
        if image_match:
            out.append(render_image(image_match.group(2)))
            idx += 1
            continue

        last = 0
        for match in MARKDOWN_IMAGE_RE.finditer(line):
            out.append(line[last : match.start()].encode(TEXT_ENCODING, errors="surrogateescape"))
            out.append(render_image(match.group(2)))
            last = match.end()
        out.append(line[last:].encode(TEXT_ENCODING, errors="surrogateescape"))
        idx += 1

    return b"".join(out)


def strip_line_ending(line: str) -> Tuple[str, str]:
    if line.endswith("\r\n"):
        return line[:-2], "\r\n"
    if line.endswith("\n") or line.endswith("\r"):
        return line[:-1], line[-1]
    return line, ""


def parse_table_row(line: str) -> Optional[List[str]]:
    body, _ = strip_line_ending(line)
    body = body.strip()
    if "|" not in body:
        return None
    if body.startswith("|"):
        body = body[1:]
    if body.endswith("|"):
        body = body[:-1]
    cells = [cell.strip() for cell in body.split("|")]
    return cells if len(cells) >= 2 else None


def is_table_separator(line: str) -> bool:
    cells = parse_table_row(line)
    if not cells:
        return False
    return all(TABLE_SEP_CELL_RE.match(cell.replace(" ", "")) for cell in cells)


def is_markdown_table_header(line: str, next_line: str) -> bool:
    cells = parse_table_row(line)
    return bool(cells and is_table_separator(next_line))


def collect_markdown_table(lines: List[str], start: int) -> Tuple[List[List[str]], int]:
    table: List[List[str]] = []
    header = parse_table_row(lines[start])
    if header:
        table.append(header)
    idx = start + 2
    while idx < len(lines):
        row = parse_table_row(lines[idx])
        if row is None:
            break
        table.append(row)
        idx += 1
    return table, idx


def render_markdown_table(rows: List[List[str]]) -> str:
    if not rows:
        return ""
    col_count = max(len(row) for row in rows)
    padded = [row + [""] * (col_count - len(row)) for row in rows]
    widths = [
        max(len(row[col]) for row in padded)
        for col in range(col_count)
    ]
    border = "+" + "+".join("-" * (width + 2) for width in widths) + "+"
    rendered: List[str] = []
    rendered.append(border)
    for row_idx, row in enumerate(padded):
        rendered.append(
            "|"
            + "|".join(f" {cell.ljust(widths[col])} " for col, cell in enumerate(row))
            + "|"
        )
        if row_idx == 0:
            rendered.append(border)
    rendered.append(border)
    return "\n".join(rendered) + "\n"


def render_markdown_stream(data: bytes, adapter: ImageAdapter) -> bytes:
    out: List[bytes] = []
    for is_kitty, chunk in split_preserving_kitty_blocks(data):
        if is_kitty:
            out.append(chunk)
        else:
            out.append(render_text_chunk(chunk, adapter.render_markdown_image))
    return b"".join(out)


def read_input(path_arg: str) -> Tuple[bytes, Path]:
    if path_arg == "-":
        return sys.stdin.buffer.read(), Path.cwd()
    path = Path(path_arg)
    with path.open("rb") as f:
        return f.read(), path.resolve().parent


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", help="Markdown/text file to send, or '-' for stdin")
    parser.add_argument("--sock", default=None, help="real vibe-bridge daemon socket")
    parser.add_argument("--plugin", default="markdown-render")
    parser.add_argument("--hold", type=float, default=10.0, help="seconds to keep session alive")
    parser.add_argument("--stdout", action="store_true", help="write adapted VT100 stream to stdout")
    parser.add_argument("--clear", action="store_true", help="clear screen and home cursor before sending")
    parser.add_argument("--image-cols", type=int, default=24, help="Kitty c= columns per image")
    parser.add_argument("--max-image-rows", type=int, default=8, help="maximum Kitty r= rows per image")
    parser.add_argument("--cell-w", type=int, default=12, help="host-side resize cell width in pixels")
    parser.add_argument("--cell-h", type=int, default=24, help="host-side resize cell height in pixels")
    parser.add_argument("--chunk-size", type=int, default=3072, help="base64 chars per Kitty chunk")
    cursor = parser.add_mutually_exclusive_group()
    cursor.add_argument("--advance-cursor", dest="advance_cursor", action="store_true", default=True)
    cursor.add_argument("--no-advance-cursor", dest="advance_cursor", action="store_false")
    return parser.parse_args()


def validate_args(args: argparse.Namespace) -> None:
    if args.image_cols <= 0:
        raise SystemExit("--image-cols must be positive")
    if args.max_image_rows <= 0:
        raise SystemExit("--max-image-rows must be positive")
    if args.cell_w <= 0 or args.cell_h <= 0:
        raise SystemExit("--cell-w and --cell-h must be positive")
    if args.chunk_size <= 0:
        raise SystemExit("--chunk-size must be positive")


def main() -> int:
    args = parse_args()
    validate_args(args)

    source, base_dir = read_input(args.input)
    adapter = ImageAdapter(
        base_dir=base_dir,
        image_cols=args.image_cols,
        max_image_rows=args.max_image_rows,
        cell_w=args.cell_w,
        cell_h=args.cell_h,
        chunk_size=args.chunk_size,
        advance_cursor=args.advance_cursor,
    )
    data = render_markdown_stream(source, adapter)
    if args.clear:
        data = b"\x1b[2J\x1b[H" + data

    if args.stdout:
        sys.stdout.buffer.write(data)
        return 0

    sock_path = args.sock or os.environ.get("VIBE_SOCK_PATH")
    if not sock_path:
        print(
            "error: set VIBE_SOCK_PATH to the real HID daemon socket, "
            "or pass --sock. Refusing to use the default mock socket.",
            file=sys.stderr,
        )
        return 2

    repo_src = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "src"))
    if repo_src not in sys.path:
        sys.path.insert(0, repo_src)
    from vibe_bridge.plugin_client import PluginClient

    with PluginClient(plugin_name=args.plugin, sock_path=sock_path) as client:
        sid = client.acquire_session()
        print(f"markdown render session sid={sid}", file=sys.stderr)
        client.send_vt100(data)
        if args.hold > 0:
            time.sleep(args.hold)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
