#!/usr/bin/env bash
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
harness="$here/bwrap/target/release/lbx-bwrap"

cargo build --release --manifest-path "$here/bwrap/Cargo.toml" >/dev/null

workspace=$(mktemp -d /tmp/lbx-edge.XXXXXX)
trap 'rm -rf "$workspace"' EXIT

run() {
    "$harness" --workspace "$workspace" -- "$@"
}

devices=$(run ls /dev | tr '\n' ' ')
for expected in null zero urandom random; do
    grep -qw "$expected" <<<"$devices" || {
        echo "edge: /dev/$expected is absent"
        exit 1
    }
done

pids=$(run sh -c 'ls /proc | grep -cE "^[0-9]+$"')
if (( pids >= 20 )); then
    echo "edge: $pids processes visible"
    exit 1
fi

inner=$(run readlink /proc/self/ns/cgroup)
outer=$(readlink /proc/self/ns/cgroup)
if [[ "$inner" == "$outer" ]]; then
    echo "edge: cgroup namespace shared"
    exit 1
fi

if run bwrap --unshare-user -- /bin/sh -c true 2>/tmp/lbx-nested.err; then
    echo "edge: nested bwrap ran against the host root"
    exit 1
fi
if ! grep -q "No such file" /tmp/lbx-nested.err; then
    echo "edge: nested bwrap failed unexpectedly"
    cat /tmp/lbx-nested.err
    exit 1
fi

start=$(date +%s%N)
run sh -c 'sleep 1 &'
end=$(date +%s%N)
elapsed=$(( (end - start) / 1000000 ))
if (( elapsed > 500 )); then
    echo "edge: background process held the sandbox ($elapsed ms)"
    exit 1
fi

echo "edge: devices, pid namespace, cgroup namespace, nested bwrap and teardown ok"
