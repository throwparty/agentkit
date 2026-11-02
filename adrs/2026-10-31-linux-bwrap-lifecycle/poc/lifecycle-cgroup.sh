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
before=$(wc -c <"$workspace/loop")

if ! systemctl --user freeze "$unit.scope"; then
    echo "cgroup: freeze unavailable"
    exit 1
fi
sleep 0.5
during=$(wc -c <"$workspace/loop")

systemctl --user thaw "$unit.scope"
sleep 0.5
after=$(wc -c <"$workspace/loop")

systemctl --user stop "$unit.scope"
wait "$runner" 2>/dev/null || true

if [[ "$before" != "$during" ]]; then
    echo "cgroup: not frozen ($before -> $during)"
    exit 1
fi
if (( after <= during )); then
    echo "cgroup: not resumed ($during -> $after)"
    exit 1
fi

echo "cgroup: scope $unit.scope frozen at $before, resumed to $after"
