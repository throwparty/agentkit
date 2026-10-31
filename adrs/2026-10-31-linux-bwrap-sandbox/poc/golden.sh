#!/usr/bin/env bash
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
harness="$here/bwrap/target/release/lbx-bwrap"

cargo build --release --manifest-path "$here/bwrap/Cargo.toml" >/dev/null
cargo build --release --features attribution --target-dir "$here/bwrap/target/attribution" --manifest-path "$here/bwrap/Cargo.toml" >/dev/null

attribution="$here/bwrap/target/attribution/release/lbx-bwrap"

workspace=$(mktemp -d /tmp/lbx-golden.XXXXXX)
trap 'rm -rf "$workspace"' EXIT
printf 'extra\n' >"$workspace/extra"

normalise() {
    sed -E "s#/nix/store/[^/]+#<STORE>#g; s#/proc/self/fd/[0-9]+#<FD>#g; s#${workspace}#<WS>#g"
}

generate() {
    env -i HOME="$HOME" PATH="$PATH" "$harness" --workspace "$workspace" --print-argv "$@" | normalise
}

generate_attribution() {
    env -i HOME="$HOME" PATH="$PATH" "$attribution" --workspace "$workspace" --print-argv "$@" | normalise
}

diff -u "$here/argv/default.txt" <(generate -- true)
diff -u "$here/argv/omit-etc.txt" <(generate_attribution --omit etc -- true)
diff -u "$here/argv/omit-shell.txt" <(generate_attribution --omit shell -- true)
diff -u "$here/argv/read-extra.txt" <(generate --read "$workspace/extra" -- true)

echo "golden: argv matches"
