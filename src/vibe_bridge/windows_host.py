"""Windows product-mode checks and adapter contract.

This module intentionally does not emulate the Linux hidraw/wrapper path on
Windows.  Windows product mode has to run as a native host process and collect
sessions through explicit adapters, while keeping the existing HID packet
protocol and board-assigned session ids.
"""

from __future__ import annotations

import platform
import os
from dataclasses import dataclass
from typing import Iterable, List


@dataclass(frozen=True)
class WindowsCheck:
    status: str
    name: str
    detail: str


@dataclass(frozen=True)
class WindowsAdapter:
    name: str
    source: str
    session_path: str
    activation_path: str
    status: str


def is_native_windows() -> bool:
    return platform.system().lower() == "windows"


def product_data_dir() -> str:
    base = os.environ.get("LOCALAPPDATA") or os.path.expanduser("~")
    return os.path.join(base, "VibeBridge")


def default_state_path() -> str:
    return os.path.join(product_data_dir(), "state.json")


def default_screen_path() -> str:
    return os.path.join(product_data_dir(), "screen.out")


def adapter_plan() -> List[WindowsAdapter]:
    return [
        WindowsAdapter(
            name="windows-cli",
            source="Windows Terminal / console-launched Codex, Claude, Gemini, local tools",
            session_path="native shim starts the tool under ConPTY and registers one session",
            activation_path="Win32 foreground window activation plus ConPTY focus metadata",
            status="first ConPTY runner implemented",
        ),
        WindowsAdapter(
            name="wsl-cli",
            source="WSL terminals launched from the Windows host",
            session_path="Windows shim launches wsl.exe with bridge env and captures ConPTY output",
            activation_path="activate the owning Windows terminal window, then replay the session",
            status="first wsl.exe launcher implemented",
        ),
        WindowsAdapter(
            name="vscode",
            source="VS Code extension-host agent sessions",
            session_path="VS Code extension calls the local bridge IPC and streams terminal/webview output",
            activation_path="VS Code command opens the owning editor, terminal, or webview",
            status="requires companion extension",
        ),
        WindowsAdapter(
            name="browser",
            source="browser-hosted agent pages",
            session_path="browser extension registers tabs through Native Messaging or local IPC",
            activation_path="browser extension focuses tab/window; desktop app only activates top window",
            status="requires companion extension",
        ),
    ]


def doctor_checks() -> List[WindowsCheck]:
    checks: List[WindowsCheck] = []
    if is_native_windows():
        checks.append(
            WindowsCheck(
                "OK",
                "platform",
                "native Windows host detected",
            )
        )
    else:
        checks.append(
            WindowsCheck(
                "FAIL",
                "platform",
                "not running on native Windows; WSL/Linux is no longer the product host path",
            )
        )

    checks.append(
        WindowsCheck(
            "OK",
            "protocol",
            "existing session_id/HID/VT100/WINDOW_ACTIVATE protocol remains the product contract",
        )
    )
    if is_native_windows():
        try:
            from .transport_win_hid import list_win_hid_devices

            devices = list_win_hid_devices()
        except Exception as exc:
            checks.append(
                WindowsCheck(
                    "FAIL",
                    "usb-hid",
                    f"native Windows HID enumeration failed: {exc}",
                )
            )
        else:
            if devices:
                checks.append(
                    WindowsCheck(
                        "OK",
                        "usb-hid",
                        f"found {len(devices)} Vibe HID device(s)",
                    )
                )
            else:
                checks.append(
                    WindowsCheck(
                        "FAIL",
                        "usb-hid",
                        "no Vibe HID 359f:2120 device found",
                    )
                )
    else:
        checks.append(
            WindowsCheck(
                "WARN",
                "usb-hid",
                "native Windows HID transport is implemented but cannot run from WSL/Linux",
            )
        )
    checks.append(
        WindowsCheck(
            "WARN",
            "adapters",
            "CLI/WSL ConPTY adapters exist; VS Code/browser adapters still need companion extensions",
        )
    )
    if is_native_windows():
        try:
            from .agent_scanner import _windows_iter_processes

            procs = _windows_iter_processes()
        except Exception as exc:
            checks.append(
                WindowsCheck(
                    "FAIL",
                    "agent-scanner",
                    f"Win32 ToolHelp32 process enumeration failed: {exc}",
                )
            )
        else:
            agents = sum(
                1
                for _pid, exe in procs
                if exe.lower() in ("codex.exe", "claude.exe", "claude-code.exe")
            )
            checks.append(
                WindowsCheck(
                    "OK",
                    "agent-scanner",
                    f"ToolHelp32 sees {len(procs)} processes ({agents} known agent(s))",
                )
            )
    else:
        checks.append(
            WindowsCheck(
                "WARN",
                "agent-scanner",
                "auto-hook scanner is implemented; ToolHelp32 path only runs on native Windows",
            )
        )
    return checks


def has_failures(checks: Iterable[WindowsCheck]) -> bool:
    return any(check.status == "FAIL" for check in checks)
