#!/bin/sh
# Egress grants against local servers. No external dependency.
# Usage: egress.sh <prototype-binary> <label>
#
# Every probe declares the verdict it expects. A wrong verdict fails the run, so
# "REFUSED" — traffic that reached the network stack — cannot be reported as a
# pass.
set -eu
POC=$(cd "$(dirname "$0")" && pwd)
BIN=$1
LABEL=$2
WS=/private/tmp/lbx-egress-$LABEL
# Ephemeral ports: a stale listener from an earlier run must not be able to
# answer for this one.
PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
OTHER=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
FAILURES=0

case "$LABEL" in
    ""|*[!A-Za-z0-9_-]*)
        echo "egress.sh: label must match [A-Za-z0-9_-]+" >&2
        exit 64
        ;;
esac

case "$WS" in
    /private/tmp/lbx-egress-*) ;;
    *) echo "egress.sh: refusing to use $WS" >&2; exit 64 ;;
esac
case "$BIN" in
    /*) ;;
    *) BIN=$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN") ;;
esac

command -v python3 >/dev/null 2>&1 || { echo "egress.sh: python3 missing" >&2; exit 70; }

FIXTURE=$POC/fixture/target/release/fixture
if [ ! -x "$FIXTURE" ]; then
    echo "egress.sh: build the fixture first (cargo build --release in poc/fixture)" >&2
    exit 70
fi

rm -rf "$WS"
mkdir -p "$WS"
cp "$FIXTURE" "$WS/probe"
FIXTURE="$WS/probe"
cd "$WS"

python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>&1 &
SERVER=$!
python3 -m http.server "$OTHER" --bind 127.0.0.1 >/dev/null 2>&1 &
OTHER_SERVER=$!
trap 'kill "$SERVER" "$OTHER_SERVER" 2>/dev/null || true; rm -rf "$WS"' EXIT INT TERM

kill -0 "$SERVER" 2>/dev/null || { echo "egress.sh: server $PORT exited" >&2; exit 70; }
kill -0 "$OTHER_SERVER" 2>/dev/null || { echo "egress.sh: server $OTHER exited" >&2; exit 70; }

i=0
until /usr/bin/curl -sS --max-time 1 -o /dev/null "http://127.0.0.1:$PORT/" \
    && /usr/bin/curl -sS --max-time 1 -o /dev/null "http://127.0.0.1:$OTHER/"; do
    i=$((i + 1))
    [ "$i" -gt 20 ] && { echo "egress.sh: servers did not start" >&2; exit 70; }
    sleep 0.2
done

# expect <label> <expected> <grant> <probe args...>
expect() {
    label=$1
    wanted=$2
    grant=$3
    shift 3
    # shellcheck disable=SC2086
    actual=$("$BIN" --tag "lbx-$(basename "$0" .sh)-$(printf %s "$LABEL")" --workspace "$WS" $grant -- /bin/sh -c "$FIXTURE $*" 2>/dev/null | tail -1)
    if [ "$actual" = "$wanted" ]; then
        echo "$LABEL $label: $actual"
    else
        echo "$LABEL $label: FAILED (wanted $wanted, got ${actual:-nothing})" >&2
        FAILURES=$((FAILURES + 1))
    fi
}

expect "granted-port  " ok           "--connect localhost:$PORT"  "--probe-connect 127.0.0.1:$PORT"
expect "second-port   " eperm        "--connect localhost:$PORT"  "--probe-connect 127.0.0.1:$OTHER"
expect "no-grant      " eperm        ""                            "--probe-connect 127.0.0.1:$PORT"
# Control: the host must be able to resolve, or dns-failed means nothing.
control=$("$FIXTURE" --probe-resolve example.com 2>/dev/null | tail -1)
case "$control" in
    resolved:*)
        echo "$LABEL resolver-control: host resolves"
        expect "resolver      " dns-failed "" "--probe-resolve example.com"
        ;;
    *)
        echo "$LABEL resolver: SKIPPED (host cannot resolve; dns denial not measurable)"
        ;;
esac
# Host specificity cannot be measured: the profile language accepts only `*`
# and `localhost` as a host token, so an IP-addressed grant does not compile.
# Recorded in findings; the port case above is the only discrimination available.

exit "$FAILURES"
