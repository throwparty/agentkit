#!/usr/bin/env bash
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
harness="$here/bwrap/target/release/lbx-bwrap"

cargo build --release --manifest-path "$here/bwrap/Cargo.toml" >/dev/null

if ! systemctl --user show-environment >/dev/null 2>&1; then
    echo "cgroup: no user manager; use lifecycle.sh"
    exit 69
fi

workspace=$(mktemp -d /tmp/lbx-cgroup.XXXXXX)
host_home=$(mktemp -d /tmp/lbx-cgroup-home.XXXXXX)
unit="lbx-poc-$$"
cleanup() {
    systemctl --user thaw "$unit.scope" 2>/dev/null || true
    systemctl --user stop "$unit.scope" 2>/dev/null || true
    rm -rf "$workspace" "$host_home"
}
trap cleanup EXIT

loop='while :; do echo x >> /src/loop; sleep 0.2; done'
systemd-run --user --scope --quiet --unit="$unit" -- \
    "$harness" --new-session --workspace "$workspace" \
    --env "HOST_HOME=$host_home" -- bash -c "$loop" &
runner=$!

for _ in $(seq 50); do
    if systemctl --user is-active --quiet "$unit.scope"; then
        break
    fi
    sleep 0.1
done

sleep 1
if [[ ! -f "$workspace/loop" ]]; then
    echo "cgroup: the sandbox never started"
    exit 1
fi

if ! systemctl --user freeze "$unit.scope"; then
    echo "cgroup: freeze unavailable"
    exit 1
fi

sleep 0.3
frozen_before=$(wc -c <"$workspace/loop")
sleep 0.5
frozen_after=$(wc -c <"$workspace/loop")

systemctl --user thaw "$unit.scope"
sleep 0.3
resumed=$(wc -c <"$workspace/loop")
sleep 0.5
resumed_after=$(wc -c <"$workspace/loop")

systemctl --user stop "$unit.scope"
wait "$runner" 2>/dev/null || true

sleep 0.3
stopped_before=$(wc -c <"$workspace/loop")
sleep 0.3
stopped_after=$(wc -c <"$workspace/loop")

if [[ "$frozen_before" != "$frozen_after" ]]; then
    echo "cgroup: not frozen ($frozen_before -> $frozen_after)"
    exit 1
fi
if (( resumed_after <= frozen_after )); then
    echo "cgroup: not resumed ($frozen_after -> $resumed_after)"
    exit 1
fi
if [[ "$stopped_before" != "$stopped_after" ]]; then
    echo "cgroup: still writing after stop ($stopped_before -> $stopped_after)"
    exit 1
fi

echo "cgroup: scope $unit.scope frozen at $frozen_before, resumed to $resumed_after"
