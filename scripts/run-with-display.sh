#!/bin/sh
set -e

# Run a command under a virtual X11 display (Xvfb) for headless GUI tests.
if command -v xvfb-run >/dev/null 2>&1; then
    exec xvfb-run -a "$@"
fi

if command -v Xvfb >/dev/null 2>&1; then
    # Fallback when Xvfb binary is installed but xvfb-run wrapper script is not (e.g. Fedora)
    Xvfb :99 -screen 0 1024x768x24 -nolisten tcp >/dev/null 2>&1 &
    XVFB_PID=$!
    trap 'kill $XVFB_PID 2>/dev/null || true' EXIT INT TERM
    sleep 1
    DISPLAY=:99 "$@"
else
    # No virtual display server available; run directly
    exec "$@"
fi
