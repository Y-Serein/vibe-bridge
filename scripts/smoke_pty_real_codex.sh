#!/usr/bin/env bash
# Manual smoke test for vibe-bridge PTY mode with the real codex CLI.
#
# Run this in a real terminal (not piped through tmux/screen if you can avoid it)
# so isatty() returns true on stdin/stdout. The wrapper will pick PTY mode
# automatically, fork+pty-spawn real codex, and tee its output through the
# bridge daemon as CMD_VT100_STREAM.
#
# Pass on success:
#   - codex behaves normally (input echoes, Ctrl-C works, exit code propagates)
#   - tail -f /tmp/vibe-bridge-screen.out shows the same bytes you see on screen
#   - vibe-bridge sessions shows a session with plugin=codex
#
# Bail out at any point with Ctrl-C; the daemon stays up.

set -e

REPO_DIR="${REPO_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
echo "[smoke] vibe-bridge repo: $REPO_DIR"

# 1. Make sure no stale daemon is running. (Skip if you want to keep one around.)
echo "[smoke] killing any old daemon..."
pkill -f "vibe_bridge.main.*daemon" 2>/dev/null || true
sleep 0.2
rm -f /tmp/vibe-bridge*.sock /tmp/vibe-bridge*.json /tmp/vibe-bridge*.out /tmp/vibe-bridge*.log

# 2. Put bin/ first on PATH so `codex` resolves to the wrapper.
export PATH="$REPO_DIR/bin:$PATH"
echo "[smoke] PATH lookup:"
which -a codex | sed 's/^/    /'

# 3. Open a tail window for the screen file in another terminal:
echo
echo "[smoke] In another terminal, run:"
echo "    tail -f /tmp/vibe-bridge-screen.out"
echo
echo "[smoke] In a third terminal, run (any time):"
echo "    PYTHONPATH=$REPO_DIR/src python3 -m vibe_bridge.main sessions"
echo
echo "[smoke] When ready, hit Enter to launch codex via the wrapper..."
read -r

# 4. Run real codex through the wrapper. PTY mode is auto-selected because
#    stdin & stdout are TTYs.
codex "$@"
RC=$?

echo
echo "[smoke] codex exited with rc=$RC"
echo "[smoke] check the tail and the sessions output above."
echo "[smoke] state dump:"
PYTHONPATH="$REPO_DIR/src" python3 -m vibe_bridge.main sessions || true
