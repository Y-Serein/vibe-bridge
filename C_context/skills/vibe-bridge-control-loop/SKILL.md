---
name: vibe-bridge-control-loop
description: Use this skill whenever taking over vibe-bridge, debugging WSL/Windows daemon/HID/session issues, recovering codex or claude after wrapper breakage, or designing terminal mirroring for the AIKB board. It forces a control-loop workflow, protects normal codex/claude startup, separates passive discovery from true PTY/ConPTY terminal capture, and preserves handoff/memory for 3-day continuity.
---

# Vibe Bridge Control Loop

Use this skill for `vibe-bridge` work involving Windows daemon, WSL distros, Codex/Claude sessions, HID, board SID, terminal replay, or permission approval.

## Non-Negotiable Goal

The product goal is not "find a session" or "show a summary". The observable goal is:

- normal `codex` / `claude` startup remains intact;
- board session count does not churn or climb unexpectedly;
- when terminal mirroring is claimed, the board receives real PTY/ConPTY VT100 bytes, not transcript summaries;
- Claude permission approval remains a separate hook-driven allow/deny path.

## Required Intake

Start with read-only checks:

1. Read project rules and handoff:
   - `AGENTS.md`
   - `HANDOFF.md`
   - `C_context/MEMORY.md`
   - `/home/rv_nano/Sipeed/C_context/KNOWN_FAILURES.md`
2. Run preflight:
   - `python3 /home/rv_nano/Sipeed/T_tools/agent_preflight.py --project vibe-bridge`
3. Check current working tree:
   - `git status --short`
4. Check WSL CLI safety before touching install logic:
   - `ls -la ~/.local/bin/codex ~/.local/bin/claude`
   - `readlink -f ~/.local/bin/codex ~/.local/bin/claude`
   - `codex --version`
   - `claude --version`
5. If the user mentions `slam`, ask for or inspect the same commands inside the `slam` distro. Do not assume `rv_nano` and `slam` have the same home or installed binaries.

## Control Loop Report

Always report in this shape before editing:

```markdown
### 目标
- 可观察结果是什么。

### 状态
- 当前 repo、daemon、HID、WSL CLI、board session 的证据。

### 误差
- 目标和现状差在哪里。

### 控制动作
- 本轮只控制哪些变量。

### 反馈
- 用什么日志、命令、板端行为作为反馈。

### 修正
- 如果反馈不一致，下一步怎么收敛。

### 验证
- 最小验证命令和预期输出。

### 沉淀
- 是否需要更新 HANDOFF / MEMORY。
```

## Architecture Rules

- Do not default to WSL wrapper installation. `install-windows` must not modify `~/.local/bin/codex` or `~/.local/bin/claude` unless the user explicitly chooses an opt-in WSL install path.
- Treat Windows native daemon as the product HID owner. WSL is a development/test/control environment, not the default HID owner.
- Treat board-assigned SID as authoritative. Host code must not invent final SIDs.
- Do not conflate these three channels:
  - passive discovery: transcript/process/window evidence; useful for listing and summaries;
  - terminal mirroring: PTY/ConPTY raw VT100 byte stream; required for 1:1 board display;
  - permission approval: Claude Code hook `PreToolUse` -> daemon -> board -> daemon -> hook response.
- Pure passive scanning cannot guarantee "terminal displays exactly what the board displays". If that is the goal, design or use an explicit `capture-shell` / `vibe-terminal` path that captures the whole shell via ConPTY.
- Claude hook may be installed for permission approval, but do not use hook installation as a reason to replace the `claude` binary.
- Codex currently has no confirmed Claude-style `PreToolUse` hook in this project. A normal Codex session without capture may not be real-time discoverable or controllable.
- A network gateway/API proxy can help with semantic messages, token usage, auditing, or permission metadata, but it cannot reconstruct terminal TUI state. Do not present gateway interception as a replacement for PTY/ConPTY VT100 capture.

## Terminal Capture Failure Triage

When `install-windows --terminal-profiles` reports wrapped profiles but the board stays empty, use logs to split the chain:

1. Open `%LOCALAPPDATA%\vibe-bridge\vb-daemon.log`.
2. Look for these markers in order:
   - `[terminal-shim] start`: Windows Terminal profile actually launched the shim.
   - `[register] terminal/...`: shim connected to daemon and sent `agent.register`.
   - `[board] request session ...`: daemon enqueued `REQUEST_SESSION`.
   - `[board] session response sid=...`: board/HID replied and daemon bound the SID.
3. Interpret missing markers:
   - no `[terminal-shim] start`: inspect Windows Terminal settings paths and the actual profile commandline; the user may be opening an unwrapped profile or a different settings file.
   - `[terminal-shim] start` but no `[register]`: inspect `terminal_shim_command`, ConPTY spawn, and TCP connect to `127.0.0.1:8765`.
   - `[register]` but no `[board] request session`: inspect daemon `agent.register` handling.
   - request but no response: inspect HID/board `REQUEST_SESSION -> SESSION_RESPONSE`.
   - response but blank UI: inspect board UI focus/FIFO/VT100 replay.

Useful Windows PowerShell checks:

```powershell
Get-CimInstance Win32_Process -Filter "Name = 'vb-daemon.exe'" |
  Select-Object ProcessId, ExecutablePath, CommandLine

Get-Content "$env:LOCALAPPDATA\vibe-bridge\vb-daemon.log" -Tail 240

$paths = @(
  "$env:LOCALAPPDATA\Packages\Microsoft.WindowsTerminal_8wekyb3d8bbwe\LocalState\settings.json",
  "$env:LOCALAPPDATA\Packages\Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe\LocalState\settings.json",
  "$env:LOCALAPPDATA\Microsoft\Windows Terminal\settings.json"
)
foreach ($p in $paths) {
  if (Test-Path $p) {
    "=== $p ==="
    Select-String -Path $p -Pattern "terminal-shim|vb-daemon|commandline|name" -Context 0,1
  }
}
```

Be explicit about shell context: `$env:WT_PROFILE_ID` is PowerShell syntax, not WSL bash syntax.

## Blank-After-Flash Triage

Use this when the user reports that a session appears, entering it shows terminal content briefly, then the board becomes a black empty screen with only a blinking `_` cursor.

First, preserve the exact symptom:

- Do not call it a white screen if the user says black/empty/backlight only.
- Do not restart from "no session" triage if a session and first frame already appeared.
- Do not keep patching ANSI parsing after two visual attempts fail. Add counters.

Split the failure before fixing:

1. Host daemon:
   - focused sid and agent key;
   - whether the sid is attached to a parent terminal;
   - replay bytes at focus;
   - live stream bytes after focus;
   - printable byte count;
   - clear/alt-screen control sequence samples around `ESC[2J`;
   - whether attached agent buffer is empty.
2. Board `aikb_hid_input`:
   - active sid;
   - VT100 packet count and byte count per sid;
   - last packet timestamp/length;
   - bytes written to LCD FIFO.
3. Board `aikb_lcd_ui`:
   - received byte count;
   - printable char count;
   - parser state;
   - full-clear and pending-clear counts;
   - non-empty cell count;
   - viewport top and effective viewport top;
   - focused sid.

Interpretation:

- no host live bytes after focus: fix focus/replay/attached stream.
- host bytes present but no `aikb_hid_input` packets: fix HID/session routing.
- input packets present but no LCD FIFO bytes: fix board bridge active sid/fifo path.
- LCD bytes present but printable count stays zero: fix parser/string-state handling.
- printable/non-empty cells appear then drop to zero after clear: fix clear/alt-screen/repaint behavior.
- non-empty cells exist but visible screen is empty: fix viewport/effective top.

Do not ask the user to burn another firmware unless the new build adds differentiated evidence or a targeted fix tied to evidence.

## Recovery Playbook

If `codex` or `claude` fails with a `vb-daemon.exe: not found` wrapper error:

1. Inspect, do not guess:
   ```bash
   ls -la ~/.local/bin/codex ~/.local/bin/claude
   sed -n '1,20p' ~/.local/bin/codex 2>/dev/null
   sed -n '1,20p' ~/.local/bin/claude 2>/dev/null
   readlink -f ~/.local/bin/codex ~/.local/bin/claude
   ```
2. For Codex, restore to the real Node/Codex binary:
   ```bash
   find ~/.nvm/versions/node -path '*/bin/codex' -o -path '*/vendor/*/bin/codex' 2>/dev/null
   ln -sfn <real-codex-path> ~/.local/bin/codex
   codex --version
   ```
3. For Claude, find real ELF binaries under `~/.local/share/claude/versions`:
   ```bash
   ls -la ~/.local/share/claude/versions
   file ~/.local/share/claude/versions/*
   find ~/.local/share/claude/versions -maxdepth 1 -type f -perm -111 -size +1000000c
   ln -sfn <real-claude-version> ~/.local/bin/claude
   claude --version
   ```
4. Preserve bad wrappers as `*.vibe-bridge-wrapper-broken` for audit. Do not delete them unless the user asks.
5. Clean misleading `real-bin` symlinks if they point at wrapper files.

## Validation Ladder

Use the smallest validation that proves the current claim:

- CLI recovery:
  - `codex --version`
  - `claude --version`
- Rust daemon edit:
  - `cargo fmt --package vb-daemon --check`
  - `cargo test -p vb-daemon`
  - `cargo check -p vb-daemon --target x86_64-pc-windows-gnu`
  - `git diff --check -- <touched-files>`
- Windows install behavior:
  - user-run `cargo run -p vb-daemon -- install-windows`
  - expected: `wsl install: skipped`
- Daemon/HID:
  - Windows log tail: `%LOCALAPPDATA%\vibe-bridge\vb-daemon.log`
  - expected HID path includes `vid_359f&pid_2120`
  - session count should not churn or climb continuously.
- True terminal mirror:
  - must involve launched/captured PTY/ConPTY bytes;
  - board should show recent VT100 state after focus and update on new output.
- Permission approval:
  - Claude `PreToolUse` creates board `PERM`;
  - `CONFIRM` returns allow;
  - `KEY0` / reject returns deny;
  - daemon logs show hook/passive agent IDs align.

## Handoff Requirements

At the end of a long session, update:

- `HANDOFF.md` top section with:
  - 30-second current state;
  - latest user-tested facts;
  - next smallest closed loop;
  - explicit "do not repeat" pitfalls.
- `C_context/MEMORY.md` with:
  - user preferences learned;
  - mistakes and best practices;
  - current constraints and traps;
  - a reusable prompt for the next session.

Keep historical notes below the latest section, but make the top section override older architecture assumptions.
