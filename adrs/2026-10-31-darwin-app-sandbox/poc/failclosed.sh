#!/bin/sh
# Fail-closed behaviour at the binary boundary: every invalid input must exit
# non-zero and must not run the command.
# Usage: failclosed.sh <prototype-binary> <label>
set -eu
BIN=$1
LABEL=$2
WS=/private/tmp/lbx-failclosed-$LABEL
SENTINEL=$WS/lbx-sentinel
FAILURES=0

case "$LABEL" in
    ""|*[!A-Za-z0-9_-]*)
        echo "failclosed.sh: label must match [A-Za-z0-9_-]+" >&2
        exit 64
        ;;
esac

case "$WS" in
    /private/tmp/lbx-failclosed-*) ;;
    *) echo "failclosed.sh: refusing to use $WS" >&2; exit 64 ;;
esac
case "$BIN" in
    /*) ;;
    *) BIN=$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN") ;;
esac

rm -rf "$WS"
mkdir -p "$WS"
cd "$WS"

expect_failure() {
    label=$1
    wanted=$2
    shift 2
    rm -f "$SENTINEL"
    status=0
    "$@" >/dev/null 2>&1 || status=$?
    if [ "$status" -eq 0 ]; then
        echo "$LABEL $label: FAILED (exit 0)" >&2
        FAILURES=$((FAILURES + 1))
    elif [ "$status" -ne "$wanted" ]; then
        echo "$LABEL $label: FAILED (wanted exit $wanted, got $status)" >&2
        FAILURES=$((FAILURES + 1))
    elif [ -e "$SENTINEL" ]; then
        echo "$LABEL $label: FAILED (command ran)" >&2
        FAILURES=$((FAILURES + 1))
    else
        echo "$LABEL $label: exit $status, command did not run"
    fi
}

TOUCH="/usr/bin/touch $SENTINEL"

expect_failure "no-workspace   " 64 "$BIN" -- /bin/sh -c "$TOUCH"
expect_failure "relative-ws    " 64 "$BIN" --tag "lbx-$(basename "$0" .sh)-$(printf %s "$LABEL")" --workspace relative -- /bin/sh -c "$TOUCH"
expect_failure "dotdot-ws      " 64 "$BIN" --tag "lbx-$(basename "$0" .sh)-$(printf %s "$LABEL")" --workspace "$WS/../elsewhere" -- /bin/sh -c "$TOUCH"
expect_failure "unknown-arg    " 64 "$BIN" --tag "lbx-$(basename "$0" .sh)-$(printf %s "$LABEL")" --workspace "$WS" --nope -- /bin/sh -c "$TOUCH"
expect_failure "relative-cmd   " 64 "$BIN" --tag "lbx-$(basename "$0" .sh)-$(printf %s "$LABEL")" --workspace "$WS" -- sh -c "$TOUCH"
expect_failure "bad-connect    " 64 "$BIN" --tag "lbx-$(basename "$0" .sh)-$(printf %s "$LABEL")" --workspace "$WS" --connect 'x") (allow file-write* (subpath "/"))' -- /bin/sh -c "$TOUCH"
expect_failure "no-command     " 64 "$BIN" --tag "lbx-$(basename "$0" .sh)-$(printf %s "$LABEL")" --workspace "$WS"

rm -rf "$WS"
exit "$FAILURES"
