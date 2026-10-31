#!/bin/sh
# Run the fixture workload and the hostile checks under one prototype.
# Usage: run.sh <prototype-binary> <label> [custom|nono]
set -eu
POC=$(cd "$(dirname "$0")" && pwd)
BIN=$1
LABEL=$2
MODE=${3:-custom}
WS=/private/tmp/lbx-$LABEL

case "$LABEL" in
    ""|*[!A-Za-z0-9_-]*)
        echo "run.sh: label must match [A-Za-z0-9_-]+" >&2
        exit 64
        ;;
esac

case "$WS" in
    /private/tmp/lbx-*) ;;
    *) echo "run.sh: refusing to use $WS" >&2; exit 64 ;;
esac
case "$BIN" in
    /*) ;;
    *) BIN=$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN") ;;
esac

REAL_HOME=$HOME
FAKE_HOME=/private/tmp/lbx-home-$LABEL

PROFILE=/private/tmp/lbx-$LABEL-profile.sbpl
rm -rf "$WS" "$FAKE_HOME"
rm -f "$PROFILE"
mkdir -p "$WS/fixture" "$WS/tmp" "$FAKE_HOME/.ssh" "$FAKE_HOME/Library/Keychains"
cp "$POC/fixture/Cargo.toml" "$WS/fixture/"
cp -R "$POC/fixture/src" "$WS/fixture/src"

# A synthetic home outside the workspace keeps the hostile checks hermetic and
# still measurable: the credential fixture must exist, and writes to $HOME must
# land outside every grant.
printf 'lbx-fixture-key\n' > "$FAKE_HOME/.ssh/id_ed25519"
printf 'lbx-fixture-keychain\n' > "$FAKE_HOME/Library/Keychains/login.keychain-db"
HOME="$FAKE_HOME"
TMPDIR="$WS/tmp"
LBX_RUN_LABEL="$LABEL"
export HOME TMPDIR LBX_RUN_LABEL

# Start inside the workspace: a cwd outside the grants makes getcwd fail.
cd "$WS"

set --
for path in "$REAL_HOME/.cargo/registry" "$REAL_HOME/.cargo/git"; do
    if [ -d "$path" ]; then
        set -- "$@" --read "$path"
    fi
done

echo "## workload"
if [ "$MODE" = custom ]; then
    "$BIN" "$@" --workspace "$WS" --tag "$LABEL" --dump-profile "$PROFILE" -- \
        /bin/sh -c 'cd "$0/fixture" && cargo build --quiet && cargo test && cargo run --quiet' "$WS"
else
    "$BIN" "$@" --workspace "$WS" --tag "$LABEL" -- \
        /bin/sh -c 'cd "$0/fixture" && cargo build --quiet && cargo test && cargo run --quiet' "$WS"
fi

echo "## hostile"
HOSTILE="$WS/fixture/target/debug/fixture --hostile"
if [ "$MODE" = custom ]; then
    # shellcheck disable=SC2086
    "$BIN" --workspace "$WS" --tag "$LABEL" -- /bin/sh -c "$HOSTILE"
else
    # shellcheck disable=SC2086
    "$BIN" --workspace "$WS" --tag "$LABEL" -- /bin/sh -c "$HOSTILE"
fi
