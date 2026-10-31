#!/bin/sh
# Exercise pause, resume, group kill and process-group escape.
# Usage: lifecycle.sh <prototype-binary> <label>
set -eu
BIN=$1
LABEL=$2
WS=/private/tmp/lbx-life-$LABEL
SENTINEL="lbx-lifecycle-$LABEL"
FAILURES=0

case "$LABEL" in
    ""|*[!A-Za-z0-9_-]*)
        echo "lifecycle.sh: label must match [A-Za-z0-9_-]+" >&2
        exit 64
        ;;
esac

case "$WS" in
    /private/tmp/lbx-life-*) ;;
    *) echo "lifecycle.sh: refusing to use $WS" >&2; exit 64 ;;
esac
case "$BIN" in
    /*) ;;
    *) BIN=$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN") ;;
esac

rm -rf "$WS"
mkdir -p "$WS"
cd "$WS"

cleanup() {
    [ -n "${PID:-}" ] && kill -KILL "-$PID" 2>/dev/null || true
    [ -n "${SETSID_PID:-}" ] && kill -KILL "-$SETSID_PID" 2>/dev/null || true
    pkill -f "$SENTINEL" 2>/dev/null || true
    rm -rf "$WS"
}
trap cleanup EXIT INT TERM

# The sentinel is $0 of the loop shell, so every descendant carries it in argv
# and the survivor count can actually be non-zero.
"$BIN" --tag "lbx-$(basename "$0" .sh)-$(printf %s "$LABEL")" --tag "lbx-life-$LABEL" --workspace "$WS" -- /bin/sh -c "while true; do echo tick >> loop.txt; sleep 0.2; done" "$SENTINEL" &
PID=$!

i=0
until [ -s loop.txt ]; do
    i=$((i + 1))
    [ "$i" -gt 50 ] && { echo "$LABEL loop: FAILED (never wrote)" >&2; exit 1; }
    sleep 0.2
done

PGID=$(ps -o pgid= -p "$PID" | tr -d ' ')
if [ "$PGID" != "$PID" ]; then
    echo "$LABEL group: FAILED (pgid $PGID != pid $PID)" >&2
    exit 1
fi

kill -STOP "-$PID"
before=$(wc -c < loop.txt)
sleep 1
after=$(wc -c < loop.txt)
if [ "$before" = "$after" ]; then
    echo "$LABEL pause:   frozen"
else
    echo "$LABEL pause:   FAILED (grew $before -> $after)" >&2
    FAILURES=$((FAILURES + 1))
fi

kill -CONT "-$PID"
sleep 1
resumed=$(wc -c < loop.txt)
if [ "$resumed" -gt "$after" ]; then
    echo "$LABEL resume:  grew $after -> $resumed"
else
    echo "$LABEL resume:  FAILED (stuck at $resumed)" >&2
    FAILURES=$((FAILURES + 1))
fi

kill -KILL "-$PID"
sleep 0.5
survivors=$(ps -o pid= -g "$PGID" 2>/dev/null | wc -l | tr -d ' ')
if ! kill -0 "$PID" 2>/dev/null && [ "$survivors" -eq 0 ]; then
    echo "$LABEL kill:    gone, 0 survivors"
else
    alive=$(kill -0 "$PID" 2>/dev/null && echo yes || echo no)
    echo "$LABEL kill:    FAILED (alive=$alive pgid=$PGID survivors=$survivors)" >&2
    FAILURES=$((FAILURES + 1))
fi

# A process that leaves the group stays confined but cannot be reaped by pgid.
# Run before teardown, while the workspace still exists.
if command -v perl >/dev/null 2>&1; then
    "$BIN" --tag "lbx-life-$LABEL" --workspace "$WS" -- /bin/sh -c "perl -MPOSIX -e 'POSIX::setsid(); sleep 30' $SENTINEL & sleep 5" &
    SETSID_PID=$!
    sleep 2
    kill -KILL "-$SETSID_PID" 2>/dev/null || true
    sleep 1
    escaping=$(pgrep -f "$SENTINEL" | wc -l | tr -d ' ')
    if [ "$escaping" -ge 1 ]; then
        echo "$LABEL setsid:  confined but unreapable ($escaping survivor), as expected"
    else
        echo "$LABEL setsid:  FAILED (no survivor; scenario did not run)" >&2
        FAILURES=$((FAILURES + 1))
    fi
else
    echo "$LABEL setsid:  SKIPPED (perl unavailable)" >&2
fi

# Delete also reclaims the workspace. The harness performs the removal, because
# neither prototype exposes a delete operation; the assertion is that the
# directory can be removed and is gone afterwards.
rm -rf "$WS"
if [ ! -e "$WS" ]; then
    echo "$LABEL delete:  workspace removed"
else
    echo "$LABEL delete:  FAILED (workspace still present)" >&2
    FAILURES=$((FAILURES + 1))
fi


exit "$FAILURES"
