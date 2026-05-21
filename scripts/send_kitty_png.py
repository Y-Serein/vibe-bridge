#!/usr/bin/env python3
"""Send a minimal Kitty graphics PNG sequence through vibe-bridge or a FIFO."""

from __future__ import annotations

import argparse
import base64
import os
import struct
import sys
import time
import zlib


def make_test_png(width: int = 64, height: int = 32) -> bytes:
    rows = []
    for y in range(height):
        row = bytearray([0])
        for x in range(width):
            r = 255 if x < width // 2 else 32
            g = 64 + (y * 160 // max(1, height - 1))
            b = 32 + (x * 180 // max(1, width - 1))
            a = 255
            row.extend((r, g, b, a))
        rows.append(bytes(row))
    raw = b"".join(rows)

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


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
    parts = []
    for off in range(0, len(encoded), chunk_size):
        piece = encoded[off : off + chunk_size]
        more = 1 if off + chunk_size < len(encoded) else 0
        control = (
            f"a=T,f=100,t=d,m={more},q=2,c={cols},r={rows},"
            f"i={image_id},p={placement_id}"
        )
        parts.append(b"\x1b_G" + control.encode("ascii") + b";" + piece.encode("ascii") + b"\x1b\\")
    return b"".join(parts)


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--png", default=None, help="PNG file to send; defaults to a generated test image")
    p.add_argument("--cols", type=int, default=24, help="Kitty c= display columns")
    p.add_argument("--rows", type=int, default=8, help="Kitty r= display rows")
    p.add_argument("--image-id", type=int, default=1)
    p.add_argument("--placement-id", type=int, default=1)
    p.add_argument("--chunk-size", type=int, default=3072)
    p.add_argument("--fifo", default=None, help="Write bytes directly to a board FIFO")
    p.add_argument("--stdout", action="store_true", help="Write the sequence to stdout")
    p.add_argument("--sock", default=None, help="vibe-bridge daemon socket")
    p.add_argument("--plugin", default="kitty-png-smoke")
    p.add_argument("--hold", type=float, default=10.0, help="Seconds to keep the session alive")
    p.add_argument("--no-clear", action="store_true", help="Do not clear/position before sending")
    return p.parse_args()


def main() -> int:
    args = parse_args()
    if args.chunk_size <= 0:
        raise SystemExit("--chunk-size must be positive")

    if args.png:
        with open(args.png, "rb") as f:
            png = f.read()
    else:
        png = make_test_png()

    seq = build_kitty_png_sequence(
        png,
        cols=args.cols,
        rows=args.rows,
        image_id=args.image_id,
        placement_id=args.placement_id,
        chunk_size=args.chunk_size,
    )
    prefix = b"" if args.no_clear else b"\x1b[2J\x1b[HKitty PNG smoke\r\n"
    data = prefix + seq + b"\r\npayload hidden if this line follows the image\r\n"

    if args.stdout:
        sys.stdout.buffer.write(data)
        return 0

    if args.fifo:
        with open(args.fifo, "wb", buffering=0) as f:
            f.write(data)
        return 0

    repo_src = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "src"))
    if repo_src not in sys.path:
        sys.path.insert(0, repo_src)
    from vibe_bridge.plugin_client import PluginClient

    with PluginClient(plugin_name=args.plugin, sock_path=args.sock) as client:
        sid = client.acquire_session()
        print(f"kitty png session sid={sid}", file=sys.stderr)
        client.send_vt100(data)
        if args.hold > 0:
            time.sleep(args.hold)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
