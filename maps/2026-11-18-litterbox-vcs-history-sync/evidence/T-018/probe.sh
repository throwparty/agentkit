#!/usr/bin/env bash
# T-018 — can Seatbelt deny a path inside a workspace it already allows?
#
# Mirrors the production renderer (backends/seatbelt/policy.rs) and the default
# baseline table (defaults.rs), then appends the proposed deny layer for a
# masked path. Runs the result under sandbox-exec, the documented fallback; the
# production path uses sandbox_init_with_parameters (confirmed PRESENT on this
# host), which compiles the same SBPL, so deny-rule ordering carries over.
#
# Each sandboxed command writes its output to a capture file INSIDE the
# workspace (the only host tree the profile makes writable), then the host reads
# it back. Redirecting a sandboxed command's stdout to a file outside the
# workspace would be (correctly) denied.
set -u

WORK="$(mktemp -d /tmp/lbx-t018.XXXXXX)"
WS="$(cd "$WORK" && pwd -P)"   # the backend canonicalizes every path
mkdir -p "$WS/.jj"
printf 'hello from the workspace\n' > "$WS/visible.txt"
printf 'TOP SECRET jj metadata\n' > "$WS/.jj/secret.txt"

BASELINE=(/System/Library /System/Applications /Library/Frameworks /usr/bin \
  /usr/lib /usr/share /usr/libexec /bin /sbin /private/etc \
  /private/var/db/dyld /private/var/select /dev)

PROFILE="$WORK/profile.sb"
{
  echo '(version 1)'
  echo '(deny default (with message "lbx-t018"))'
  echo '(allow process-exec)'
  echo '(allow process-fork)'
  echo '(allow process-info* (target same-sandbox))'
  echo '(allow signal (target same-sandbox))'
  echo '(allow file-read*'
  echo '  (literal "/")'
  for p in "${BASELINE[@]}"; do
    c="$(cd "$p" 2>/dev/null && pwd -P)" && echo "  (subpath \"$c\")"
  done
  echo "  (subpath \"$WS\")"
  echo ')'
  echo '(allow file-read-metadata'
  anc="$WS"
  while [ "$anc" != "/" ]; do
    echo "  (literal \"$anc\")"
    anc="$(dirname "$anc")"
  done
  echo '  (literal "/")'
  echo ')'
  echo '(allow file-write*'
  echo '  (literal "/dev/null")'
  echo "  (subpath \"$WS\")"
  echo ')'
  echo '(allow sysctl-read)'
  echo '(deny file-read*'
  echo "  (subpath \"$WS/.jj\")"
  echo ')'
  echo '(deny file-write*'
  echo "  (subpath \"$WS/.jj\")"
  echo ')'
} > "$PROFILE"

run() {
  local label="$1"; shift
  local cap="$WS/.capture"
  rm -f "$cap"
  echo "== $label =="
  echo "\$ sandbox-exec -f $PROFILE -- $*"
  sandbox-exec -f "$PROFILE" -- "$@" > "$cap" 2>&1
  echo "exit=$?"
  echo "--- sandbox output ---"
  cat "$cap" 2>/dev/null
  rm -f "$cap"
  echo
}

run "read a workspace file (expect 0)"           /bin/cat "$WS/visible.txt"
run "write a workspace file (expect 0)"          /bin/sh -c "printf written > '$WS/new.txt'"
run "read under .jj (expect nonzero, EPERM)"     /bin/cat "$WS/.jj/secret.txt"
run "write under .jj (expect nonzero)"           /bin/sh -c "printf evil > '$WS/.jj/evil.txt'"
run "list the workspace (expect .jj denied; other entries listed)" /bin/ls -la "$WS"
run "list .jj (expect nonzero)"                  /bin/ls "$WS/.jj"
run "stat .jj (expect nonzero)"                  /usr/bin/stat "$WS/.jj"
run "descendant read under .jj (expect nonzero)" /bin/sh -c "/bin/sh -c 'cat $WS/.jj/secret.txt'"

echo "== host view after the probe =="
test -f "$WS/new.txt"       && echo "allowed: new.txt was written"       || echo "unexpected: new.txt missing"
test -f "$WS/.jj/evil.txt"  && echo "LEAK: .jj/evil.txt was written"     || echo "denied: .jj/evil.txt was not written"
test -f "$WS/.jj/secret.txt" && echo "intact: .jj/secret.txt still there" || echo "unexpected: secret.txt gone"

rm -rf "$WORK"
