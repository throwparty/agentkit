#!/usr/bin/env bash
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
harness="$here/bwrap/target/release/lbx-bwrap"

cargo build --release --manifest-path "$here/bwrap/Cargo.toml" >/dev/null
cargo build --release --features attribution --target-dir "$here/bwrap/target/attribution" --manifest-path "$here/bwrap/Cargo.toml" >/dev/null

attribution="$here/bwrap/target/attribution/release/lbx-bwrap"

build_case() {
    local label=$1
    shift
    local workspace
    workspace=$(mktemp -d /tmp/lbx-attr.XXXXXX)
    cp -a "$here/fixture/." "$workspace/"
    local status=0
    if ! "$attribution" --workspace "$workspace" "$@" -- cargo build --quiet; then
        status=1
    fi
    rm -rf "$workspace"
    echo "$label: $status"
    return $status
}

build_case full
for group in etc system shell; do
    build_case "omit-$group" --omit "$group"
done

# Removing the store must reproduce a failure: the toolchain lives there.
if build_case omit-store --omit store; then
    echo "attribution: omitting the store still built"
    exit 1
fi

echo "attribution: etc, system and shell are unnecessary for this workload; the store is required"
