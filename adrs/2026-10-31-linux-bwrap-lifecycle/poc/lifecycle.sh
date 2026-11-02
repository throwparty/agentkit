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

wait_for_stopped() {
    local state
    for _ in $(seq 100); do
        state=$(ps -o stat= -p "$pid" 2>/dev/null | tr -d ' ')
        [[ -z "$state" ]] && return 1
        [[ "$state" == T* ]] && return 0
        sleep 0.05
    done
    return 1
}

wait_for_running() {
    local state
    for _ in $(seq 100); do
        state=$(ps -o stat= -p "$pid" 2>/dev/null | tr -d ' ')
        [[ -z "$state" ]] && return 1
        [[ "$state" != T* ]] && return 0
        sleep 0.05
    done
    return 1
}

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

kill -STOP -"$pid"
if ! wait_for_stopped; then
    echo "lifecycle: the process group did not stop"
    exit 1
fi
sleep 0.3
frozen_before=$(wc -c <"$workspace/loop")
sleep 0.5
frozen_after=$(wc -c <"$workspace/loop")

kill -CONT -"$pid"
if ! wait_for_running; then
    echo "lifecycle: the process group did not resume"
    exit 1
fi
sleep 0.3
resumed=$(wc -c <"$workspace/loop")
sleep 0.5
resumed_after=$(wc -c <"$workspace/loop")

kill -KILL -"$pid"
wait "$pid" 2>/dev/null || true
pid=""

sleep 0.3
stopped_before=$(wc -c <"$workspace/loop")
sleep 0.3
stopped_after=$(wc -c <"$workspace/loop")

if [[ "$frozen_before" != "$frozen_after" ]]; then
    echo "lifecycle: not frozen ($frozen_before -> $frozen_after)"
    exit 1
fi
if (( resumed_after <= frozen_after )); then
    echo "lifecycle: not resumed ($frozen_after -> $resumed_after)"
    exit 1
fi
if [[ "$stopped_before" != "$stopped_after" ]]; then
    echo "lifecycle: still writing after kill ($stopped_before -> $stopped_after)"
    exit 1
fi

echo "lifecycle: leader pgid $pgid, frozen at $frozen_before, resumed to $resumed_after, killed cleanly"
