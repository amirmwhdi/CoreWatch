#!/bin/sh
# Run a command under a virtual X11 display (Xvfb), so GUI tests work
# without a desktop session. Used by tests/run.sh.
set -e

if command -v xvfb-run >/dev/null 2>&1; then
    exec xvfb-run -a "$@"
fi

if command -v Xvfb >/dev/null 2>&1; then
    # Xvfb without the xvfb-run wrapper (Fedora): let Xvfb pick a free
    # display number and print it, instead of guessing one that may be taken.
    fifo=$(mktemp -u)
    mkfifo "$fifo"
    Xvfb -displayfd 3 -screen 0 1024x768x24 -nolisten tcp 3>"$fifo" >/dev/null 2>&1 &
    xvfb_pid=$!
    trap 'kill "$xvfb_pid" 2>/dev/null || true; rm -f "$fifo"' EXIT INT TERM
    read -r display <"$fifo"
    status=0
    DISPLAY=":$display" "$@" || status=$?
    exit "$status"
fi

# No virtual display server: run as is (on a desktop the session's display
# is used; without one, GUI tests skip themselves).
exec "$@"
