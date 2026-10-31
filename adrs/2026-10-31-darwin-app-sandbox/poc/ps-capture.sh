#!/bin/sh
# The applied profile must never appear in the process table.
# Usage: ps-capture.sh <prototype-binary> <label>
set -eu
BIN=$1
LABEL=$2
WS=/private/tmp/lbx-ps-$LABEL
FAILURES=0

case "$LABEL" in
    ""|*[!A-Za-z0-9_-]*)
        echo "ps-capture.sh: label must match [A-Za-z0-9_-]+" >&2
        exit 64
        ;;
esac

case "$WS" in
    /private/tmp/lbx-ps-*) ;;
    *) echo "ps-capture.sh: refusing to use $WS" >&2; exit 64 ;;
esac
case "$BIN" in
    /*) ;;
    *) BIN=$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN") ;;
esac

rm -rf "$WS"
mkdir -p "$WS"
cd "$WS"
trap 'rm -rf "$WS"' EXIT

# perl keeps the trailing marker in argv; `sh -c 'sleep 5' marker` is often
# optimised into `exec sleep`, which drops it.
"$BIN" --workspace "$WS" --tag "$LABEL" -- /usr/bin/perl -e 'sleep 5' lbx-ps-marker &
PID=$!
sleep 1

# Every process in the tree, whatever its argv.
ARGV=$(ps -Ao pid=,ppid=,command= 2>/dev/null | grep -c "[d]eny default" || true)
if [ "$ARGV" -ne 0 ]; then
    echo "$LABEL profile-in-argv: FAILED ($ARGV matches)" >&2
    FAILURES=$((FAILURES + 1))
else
    echo "$LABEL profile-in-argv: absent"
fi

if ps -Ao command= 2>/dev/null | grep -q "[l]bx-ps-marker"; then
    echo "$LABEL command-visible: yes"
else
    echo "$LABEL command-visible: FAILED (marker not found)" >&2
    FAILURES=$((FAILURES + 1))
fi

kill -KILL "-$PID" 2>/dev/null || kill -KILL "$PID" 2>/dev/null || true
exit "$FAILURES"
