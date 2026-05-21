"""Checkout-time import shim for the src-layout vibe_bridge package.

This lets ``python -m vibe_bridge.main`` work from the repository root before
the package has been installed in editable mode.
"""

from __future__ import annotations

from pathlib import Path

_SRC_PACKAGE = Path(__file__).resolve().parent.parent / "src" / "vibe_bridge"
if _SRC_PACKAGE.is_dir():
    __path__.append(str(_SRC_PACKAGE))

__version__ = "0.0.1"
