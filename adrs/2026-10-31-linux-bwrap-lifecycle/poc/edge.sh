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

inner_pid=$(run readlink /proc/self/ns/pid)
outer_pid=$(readlink /proc/self/ns/pid)
if [[ "$inner_pid" == "$outer_pid" ]]; then
    echo "edge: pid namespace shared"
    exit 1
fi

inner_cgroup=$(run readlink /proc/self/ns/cgroup)
outer_cgroup=$(readlink /proc/self/ns/cgroup)
if [[ "$inner_cgroup" == "$outer_cgroup" ]]; then
    echo "edge: cgroup namespace shared"
    exit 1
fi

if run bwrap --unshare-user --ro-bind /nix/store /nix/store \
    --ro-bind /bin/sh /bin/sh -- /bin/sh -c true; then
    echo "nested bwrap: permitted"
else
    echo "nested bwrap: blocked"
fi

start=$(date +%s%N)
run sh -c 'exec sleep 1'
end=$(date +%s%N)
exec_ms=$(( (end - start) / 1000000 ))
if (( exec_ms < 900 )); then
    echo "edge: exec did not hold the sandbox ($exec_ms ms)"
    exit 1
fi

run sh -c 'sleep 9876 &' >/dev/null 2>&1 || true
if pgrep -f 'sleep 9876' >/dev/null 2>&1; then
    echo "edge: background process survived teardown"
    pkill -f 'sleep 9876' 2>/dev/null || true
    exit 1
fi

if find "$workspace" -perm -4000 -print -quit | grep -q .; then
    echo "edge: setuid binary present in the workspace"
    exit 1
fi

if ! run sh -c "grep ' /src ' /proc/self/mountinfo" | grep -q nosuid; then
    echo "edge: /src is not mounted nosuid"
    exit 1
fi

echo "edge: devices, namespaces, nested bwrap, exec lifetime, teardown, setuid and nosuid ok"
