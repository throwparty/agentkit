#!/bin/sh
# Credential denial when the workspace sits inside a granted home directory.
# Usage: inside-home.sh <prototype-binary> <label>
set -eu
BIN=$1
LABEL=$2
HOME_DIR=/private/tmp/lbx-inside-$LABEL
WS=$HOME_DIR/ws
FAILURES=0

case "$HOME_DIR" in
    /private/tmp/lbx-inside-*) ;;
    *) echo "inside-home.sh: refusing to use $HOME_DIR" >&2; exit 64 ;;
esac
case "$BIN" in
    /*) ;;
    *) BIN=$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN") ;;
esac

rm -rf "$HOME_DIR"
mkdir -p "$HOME_DIR/.ssh" "$HOME_DIR/Library/Keychains" "$WS"
printf 'inside-home-key\n' > "$HOME_DIR/.ssh/id_ed25519"
printf 'inside-home-keychain\n' > "$HOME_DIR/Library/Keychains/login.keychain-db"
printf 'ordinary-notes\n' > "$HOME_DIR/notes.txt"
trap 'rm -rf "$HOME_DIR"' EXIT INT TERM
cd "$WS"

# The whole home is granted read, and the workspace is a subtree of it. The
# credential denies must still win.
result=$(HOME="$HOME_DIR" "$BIN" --tag "lbx-$(basename "$0" .sh)-$(printf %s "$LABEL")" --tag "lbx-inside-$LABEL" --workspace "$WS" --read "$HOME_DIR" -- /bin/sh -c '
    cat "$HOME/.ssh/id_ed25519" >/dev/null 2>&1 && echo key:leak || echo key:denied
    cat "$HOME/Library/Keychains/login.keychain-db" >/dev/null 2>&1 && echo kc:leak || echo kc:denied
    cat "$HOME/ws/../.ssh/id_ed25519" >/dev/null 2>&1 && echo dotdot:leak || echo dotdot:denied
    cat "$HOME/notes.txt" >/dev/null 2>&1 && echo notes:ok || echo notes:denied
' 2>&1)

echo "$result" | while read -r line; do echo "$LABEL $line"; done

echo "$result" | grep -q "key:denied" || FAILURES=$((FAILURES + 1))
echo "$result" | grep -q "kc:denied" || FAILURES=$((FAILURES + 1))
echo "$result" | grep -q "dotdot:denied" || FAILURES=$((FAILURES + 1))
echo "$result" | grep -q "notes:ok" || FAILURES=$((FAILURES + 1))

exit "$FAILURES"
