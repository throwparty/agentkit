#!/bin/bash
# T-043: Seatbelt confines a sandbox whose only inputs are a workspace and a
# platform baseline. No Nix store grant, no devShell environment.
set -u
P=$(cd "$(dirname "$0")" && pwd)
B=$P/build
H=$B/helper
WS=$B/t043-ws
rm -rf "$B"
mkdir -p "$WS/tmp"
cc -O1 -o "$H" "$P/T-043-helper.c" || exit 1
REAL_HOME=$HOME
NIXBIN=$(command -v nix)

sandboxed() {
  ( cd / && env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin HOME="$WS" TMPDIR="$WS/tmp" \
      LANG=C.UTF-8 USER="$(id -un)" REAL_HOME="$REAL_HOME" NIXBIN="$NIXBIN" \
      "$H" --workspace "$WS" -- "$@" )
}

echo "host: $(sw_vers -productVersion) ($(sw_vers -buildVersion)) $(uname -m)"
echo "workspace: $WS"
echo

echo "== 0. profile dump =="
env -i PATH=/usr/bin:/bin "$H" --workspace "$WS" --dump-profile "$B/t043.sbpl" -- /bin/sh -c 'echo profile-applied'
echo "exit=$?"
echo "grep -c nix t043.sbpl: $(grep -c nix "$B/t043.sbpl")"
echo "profile bytes: $(wc -c < "$B/t043.sbpl")"
echo

echo "== 1. /bin/sh runs =="
echo "\$ sandboxed /bin/sh -c 'echo standalone-ok'"
sandboxed /bin/sh -c 'echo standalone-ok'
echo "exit=$?"; echo

echo "== 2. write inside workspace =="
sandboxed /bin/sh -c 'echo inside > $HOME/inside.txt; cat $HOME/inside.txt'
echo "exit=$?"; echo

echo "== 3. write inside TMPDIR =="
sandboxed /bin/sh -c 'echo tmp > $TMPDIR/t.txt; cat $TMPDIR/t.txt'
echo "exit=$?"; echo

echo "== 4. write outside workspace =="
rm -f /tmp/t043-outside
sandboxed /bin/sh -c 'echo nope > /tmp/t043-outside'
echo "exit=$?"
echo "file present: $(test -e /tmp/t043-outside && echo yes || echo no)"
echo

echo "== 5. read of an ungranted host path =="
mkdir -p "$B/t043-secret"
echo secret-material > "$B/t043-secret/id_ed25519"
sandboxed /bin/sh -c "cat $B/t043-secret/id_ed25519"
echo "exit=$?"; echo

echo "== 6. Nix store binary (negative control) =="
sandboxed "$NIXBIN" --version
echo "exit=$?"; echo

echo "== 7. root literal removed: SIGABRT =="
sed '/literal "\/"/d' "$B/t043.sbpl" > "$B/t043-noliteral.sbpl"
cat > "$B/apply-only.c" <<'EOF'
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
extern int sandbox_init(const char *, unsigned long long, char **);
int main(int argc, char **argv) {
    FILE *f = fopen(argv[1], "rb");
    if (!f) return 70;
    fseek(f, 0, SEEK_END); long n = ftell(f); rewind(f);
    char *p = calloc(1, (size_t)n + 1); fread(p, 1, (size_t)n, f); fclose(f);
    char *e = NULL;
    if (sandbox_init(p, 0, &e) != 0) { fprintf(stderr, "sandbox_init: %s\n", e ? e : "?"); return 70; }
    execvp(argv[2], &argv[2]); perror("exec"); return 126;
}
EOF
cc -O1 -o "$B/apply-only" "$B/apply-only.c" || exit 1
"$B/apply-only" "$B/t043-noliteral.sbpl" /bin/sh -c 'echo no-literal-ok'
echo "exit=$?"
