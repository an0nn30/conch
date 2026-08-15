#!/usr/bin/env bash
#
# Automated memory leak checker for TermLab.
#
# Usage:
#   ./scripts/check_leaks.sh [--build]
#
# What it does:
#   1. Optionally builds the release binary (--build flag)
#   2. Launches TermLab
#   3. Waits for you to exercise the app (open/close tabs & windows)
#   4. Press Enter when ready to scan
#   5. Runs `leaks` against the process
#   6. Sends Ctrl-C to quit TermLab and shows the summary
#
# Requires macOS developer tools (for the `leaks` command).

set -euo pipefail

BINARY="./target/release/termlab"
LEAKS_LOG="/tmp/termlab-leaks-$(date +%Y%m%d-%H%M%S).txt"

if [[ "${1:-}" == "--build" ]]; then
    echo "Building release binary..."
    cargo build --release
    echo ""
fi

if [[ ! -x "$BINARY" ]]; then
    echo "Error: $BINARY not found. Run with --build or build first."
    exit 1
fi

echo "=== TermLab Memory Leak Checker ==="
echo ""
echo "Launching TermLab..."
"$BINARY" &
TERMLAB_PID=$!
sleep 2

# Verify it started
if ! kill -0 "$TERMLAB_PID" 2>/dev/null; then
    echo "Error: TermLab failed to start."
    exit 1
fi

echo "TermLab running (PID: $TERMLAB_PID)"
echo ""
echo "--- Exercise the app now ---"
echo "  - Open and close several tabs (Cmd+T, Cmd+W)"
echo "  - Open and close extra windows (Cmd+Shift+N)"
echo "  - Do this at least 5-10 times each"
echo ""
read -rp "Press Enter when ready to scan for leaks..."

echo ""
echo "Running leaks scan (this may take a moment)..."
if leaks "$TERMLAB_PID" > "$LEAKS_LOG" 2>&1; then
    LEAK_STATUS="PASS"
else
    LEAK_STATUS="LEAKS FOUND"
fi

echo ""
echo "=== Results ==="
# Extract the summary line
grep -E "^(Process|leaks for)" "$LEAKS_LOG" || true
grep -E "^[0-9]+ leak" "$LEAKS_LOG" || true
echo ""

# Show leaked bytes if any
if grep -q "total leaked bytes" "$LEAKS_LOG"; then
    grep "total leaked bytes" "$LEAKS_LOG"
fi

echo "Status: $LEAK_STATUS"
echo "Full report: $LEAKS_LOG"
echo ""

# Show top leaked call trees if leaks were found
if [[ "$LEAK_STATUS" == "LEAKS FOUND" ]]; then
    echo "--- Top leak call stacks ---"
    # Show first 60 lines of leak details
    grep -A 10 "Leak:" "$LEAKS_LOG" | head -60
    echo "..."
    echo "(see full report for details)"
    echo ""
fi

read -rp "Press Enter to quit TermLab..."
kill "$TERMLAB_PID" 2>/dev/null || true
wait "$TERMLAB_PID" 2>/dev/null || true
echo "Done."
