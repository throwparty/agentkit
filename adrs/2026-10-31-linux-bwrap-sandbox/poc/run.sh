#!/usr/bin/env bash
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
harness="$here/bwrap/target/release/lbx-bwrap"

cargo build --release --manifest-path "$here/bwrap/Cargo.toml" >/dev/null

workspace=$(mktemp -d /tmp/lbx-ws.XXXXXX)
host_home=$(mktemp -d /tmp/lbx-hosthome.XXXXXX)
trap 'rm -rf "$workspace" "$host_home"' EXIT

mkdir -p "$host_home/.ssh"
printf 'canary\n' >"$host_home/.ssh/id_ed25519"

cp -a "$here/fixture/." "$workspace/"

run() {
    "$harness" --workspace "$workspace" \
        --env "HOST_HOME=$host_home" \
        --env "HOST_PID=$$" \
        --env "CARGO_HOME=/home/sandbox/.cargo" \
        --env "RUSTUP_HOME=/home/sandbox/.rustup" \
        -- "$@"
}

run cargo build --quiet
run cargo test --quiet
run cargo run --quiet

set +e
hostile=$(run cargo run --quiet -- --hostile)
hostile_status=$?
set -e
if [[ $hostile_status -eq 69 || $hostile_status -eq 70 ]]; then
    echo "hostile: harness failed with $hostile_status"
    echo "$hostile"
    exit 1
fi
if [[ $hostile_status -ne 0 ]] || grep -q ": leak" <<<"$hostile"; then
    echo "hostile: leaked"
    echo "$hostile"
    exit 1
fi
echo "hostile: denied"

for probe in "$host_home/lbx-write-probe" "$host_home/lbx-child-probe" /nix/store/lbx-write-probe; do
    if [[ -e "$probe" ]]; then
        echo "hostile: artefact landed at $probe"
        exit 1
    fi
done

control=$(HOST_HOME="$host_home" HOST_PID=$$ "$workspace/target/debug/fixture" --hostile || true)
for expected in "write-outside: leak" "read-ssh-key: leak" "child-write: leak"; do
    if ! grep -q "$expected" <<<"$control"; then
        echo "control: missing $expected"
        echo "$control"
        exit 1
    fi
done
echo "control: leaked without the sandbox"
