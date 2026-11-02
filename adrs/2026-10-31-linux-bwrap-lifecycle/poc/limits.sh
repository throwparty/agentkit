#!/usr/bin/env bash
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
harness="$here/bwrap/target/release/lbx-bwrap"

cargo build --release --manifest-path "$here/bwrap/Cargo.toml" >/dev/null

if ! systemctl --user show-environment >/dev/null 2>&1; then
    echo "limits: no user manager"
    exit 69
fi

workspace=$(mktemp -d /tmp/lbx-limits.XXXXXX)
trap 'rm -rf "$workspace"' EXIT

run_limited() {
    local unit=$1
    shift
    systemd-run --user --scope --quiet --unit="$unit" \
        -p MemoryMax=64M -p MemorySwapMax=0 -p TasksMax=16 -- "$@"
}

unit="lbx-mem-$$"
if run_limited "$unit" "$harness" --workspace "$workspace" -- \
    python3 -c 'x = bytearray(256 * 1024 * 1024); print(len(x))' >/dev/null 2>&1; then
    echo "limits: the memory hog succeeded"
    exit 1
fi
echo "limits: memory hog bounded"

unit="lbx-tasks-$$"
if run_limited "$unit" "$harness" --workspace "$workspace" -- \
    python3 -c 'import threading,time; ts=[threading.Thread(target=time.sleep,args=(5,)) for _ in range(64)]; [t.start() for t in ts]; [t.join() for t in ts]' >/dev/null 2>&1; then
    echo "limits: the thread hog succeeded"
    exit 1
fi
echo "limits: task hog bounded"
