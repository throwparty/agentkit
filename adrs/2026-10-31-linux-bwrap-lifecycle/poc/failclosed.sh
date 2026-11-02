#!/usr/bin/env bash
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
harness="$here/bwrap/target/release/lbx-bwrap"

cargo build --release --manifest-path "$here/bwrap/Cargo.toml" >/dev/null

workspace=$(mktemp -d /tmp/lbx-failclosed.XXXXXX)
fakebin=$(mktemp -d /tmp/lbx-fakebin.XXXXXX)
trap 'rm -rf "$workspace" "$fakebin"' EXIT

printf '#!%s\necho "bubblewrap 0.7.0"\n' "$(command -v bash)" >"$fakebin/bwrap"
chmod +x "$fakebin/bwrap"

fails=0

expect() {
    local name=$1 expected=$2
    shift 2
    rm -f "$workspace/sentinel"
    set +e
    "$@" >/dev/null 2>&1
    local actual=$?
    set -e
    if [[ $actual -ne $expected ]]; then
        echo "$name: exit $actual, expected $expected"
        fails=$((fails + 1))
    fi
    if [[ -e "$workspace/sentinel" ]]; then
        echo "$name: command ran"
        fails=$((fails + 1))
    fi
}

expect "missing workspace" 64 \
    "$harness" --workspace "$workspace/absent" -- touch /src/sentinel
expect "workspace under home" 64 \
    "$harness" --workspace "$HOME" -- touch /src/sentinel
expect "read of host home" 64 \
    "$harness" --workspace "$workspace" --read "$HOME" -- touch /src/sentinel
expect "read of /etc" 64 \
    "$harness" --workspace "$workspace" --read /etc -- touch /src/sentinel
expect "unknown argument" 64 \
    "$harness" --workspace "$workspace" --write /tmp -- touch /src/sentinel
expect "no command" 64 \
    "$harness" --workspace "$workspace"
expect "missing bwrap" 69 \
    env PATH=/nonexistent "$harness" --workspace "$workspace" -- touch /src/sentinel
expect "untrusted bwrap" 69 \
    env PATH="$fakebin" "$harness" --workspace "$workspace" -- touch /src/sentinel

rm -f "$workspace/sentinel"
if ! "$harness" --workspace "$workspace" -- touch /src/sentinel; then
    echo "positive control: command did not run"
    fails=$((fails + 1))
elif [[ ! -e "$workspace/sentinel" ]]; then
    echo "positive control: sentinel absent"
    fails=$((fails + 1))
fi

linkdir=$(mktemp -d /tmp/lbx-link.XXXXXX)
ln -s "$HOME" "$linkdir/home-link"
expect "symlink into host home" 64 \
    "$harness" --workspace "$workspace" --read "$linkdir/home-link" -- touch /src/sentinel
rm -rf "$linkdir"

if [[ $fails -ne 0 ]]; then
    echo "failclosed: $fails failure(s)"
    exit 1
fi
echo "failclosed: all cases rejected and the positive control ran"
