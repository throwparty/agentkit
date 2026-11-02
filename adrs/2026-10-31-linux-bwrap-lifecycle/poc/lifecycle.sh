#!/usr/bin/env bash
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
harness="$here/bwrap/target/release/lbx-bwrap"

cargo build --release --manifest-path "$here/bwrap/Cargo.toml" >/dev/null

workspace=$(mktemp -d /tmp/lbx-lifecycle.XXXXXX)
host_home=$(mktemp -d /tmp/lbx-lifecycle-home.XXXXXX)
pid=""
cleanup() {
    if [[ -n "$pid" ]]; then
        kill -KILL -"$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    fi
    rm -rf "$workspace" "$host_home"
}
trap cleanup EXIT

loop='while :; do echo x >> /src/loop; sleep 0.2; done'
"$harness" --workspace "$workspace" --env "HOST_HOME=$host_home" -- bash -c "$loop" &
pid=$!

sleep 1
if [[ ! -f "$workspace/loop" ]]; then
    echo "lifecycle: the sandbox never started"
    exit 1
fi

pgid=$(ps -o pgid= -p "$pid" | tr -d ' ')
if [[ "$pgid" != "$pid" ]]; then
    echo "lifecycle: pid $pid is not its own process-group leader (pgid $pgid)"
    exit 1
fi

before=$(wc -c <"$workspace/loop")

kill -STOP -"$pid"
sleep 0.5
during=$(wc -c <"$workspace/loop")

kill -CONT -"$pid"
sleep 0.5
after=$(wc -c <"$workspace/loop")

kill -KILL -"$pid"
wait "$pid" 2>/dev/null || true
pid=""
sleep 0.2
final=$(wc -c <"$workspace/loop")

if [[ "$before" != "$during" ]]; then
    echo "lifecycle: not frozen ($before -> $during)"
    exit 1
fi
if (( after <= during )); then
    echo "lifecycle: not resumed ($during -> $after)"
    exit 1
fi
if (( final != after )); then
    echo "lifecycle: still writing after kill ($after -> $final)"
    exit 1
fi

echo "lifecycle: leader pgid $pgid, frozen at $before, resumed to $after, killed cleanly"
