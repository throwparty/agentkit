#!/usr/bin/env bash
# Mask a bound workspace's VCS metadata with bubblewrap.
#
# T-002 proved the directory shape, where `.jj` is a directory and a tmpfs
# hides its contents. This probe answers the open question from the hide-vcs
# ADR: whether a masked path that is a *file*, the way a linked worktree's
# `.git` is, can be masked too, and whether the directory shape still behaves.
#
# Run from the repository root:
#   bash adrs/2026-11-21-hide-vcs-metadata-from-sandboxes/evidence/T-013/probe.sh

set -uo pipefail

bwrap="${BWRAP:-bwrap}"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT

fail=0
check() {
  if [ "$2" = "$3" ]; then
    printf '  PASS %s (%s)\n' "$1" "$3"
  else
    printf '  FAIL %s: expected [%s], got [%s]\n' "$1" "$2" "$3"
    fail=1
  fi
}

field() {
  printf '%s\n' "$1" | sed -n "s/^$2=//p" | head -1
}

printf 'bwrap: %s\n' "$("$bwrap" --version)"
printf 'root: %s\n\n' "$root"

common=(
  --unshare-user --unshare-pid --unshare-ipc --unshare-uts --unshare-cgroup
  --disable-userns --hostname litterbox
  --ro-bind / / --proc /proc --dev /dev --tmpfs /tmp
)

# Case A: `.git` is a directory.
printf 'Case A: .git is a directory, masked with --tmpfs\n'
ws="$root/dir-ws"
mkdir -p "$ws/.git"
printf 'SECRET\n' >"$ws/.git/HEAD"
printf 'gitdir: /tmp/elsewhere\n' >"$ws/.git/config"
printf 'hello\n' >"$ws/README.md"

out="$("$bwrap" "${common[@]}" \
  --bind "$ws" /tmp/src --tmpfs /tmp/src/.git --chdir /tmp/src -- \
  sh -c '
    printf "readme=%s\n" "$(cat README.md)"
    if cat .git/HEAD 2>/dev/null; then printf "head=leaked\n"; else printf "head=denied\n"; fi
    printf "entries=%s\n" "$(ls -A .git 2>/dev/null | wc -l)"
    if touch .git/written 2>/dev/null; then printf "write=tmpfs\n"; else printf "write=denied\n"; fi
  ' 2>&1)"
printf '%s\n' "$out" | sed 's/^/  /'
check "readme is readable" "hello" "$(field "$out" readme)"
check "HEAD is denied" "denied" "$(field "$out" head)"
check "the directory entry is empty" "0" "$(field "$out" entries)"
check "a write lands in the tmpfs, not the host" "tmpfs" "$(field "$out" write)"
check "the host .git is untouched" "SECRET" "$(cat "$ws/.git/HEAD")"
check "no file lands on the host" "absent" \
  "$([ -e "$ws/.git/written" ] && echo present || echo absent)"
printf '\n'

# Case B: `.git` is a file, the way a linked worktree has it.
printf 'Case B: .git is a file, masked with a read-only bind over an empty file\n'
ws="$root/file-ws"
mkdir -p "$ws"
printf 'gitdir: /tmp/elsewhere\n' >"$ws/.git"
printf 'hello\n' >"$ws/README.md"
: >"$root/empty"

out="$("$bwrap" "${common[@]}" \
  --bind "$ws" /tmp/src --ro-bind "$root/empty" /tmp/src/.git --chdir /tmp/src -- \
  sh -c '
    printf "readme=%s\n" "$(cat README.md)"
    printf "bytes=%s\n" "$(wc -c < .git 2>/dev/null || echo none)"
    if printf x >> .git 2>/dev/null; then printf "write=accepted\n"; else printf "write=denied\n"; fi
  ' 2>&1)"
printf '%s\n' "$out" | sed 's/^/  /'
check "readme is readable" "hello" "$(field "$out" readme)"
check "the gitfile is empty" "0" "$(field "$out" bytes)"
check "a write is denied" "denied" "$(field "$out" write)"
check "the host gitfile is untouched" "gitdir: /tmp/elsewhere" "$(cat "$ws/.git")"
printf '\n'

# Case C: the control. Without a mask the same workspace leaks.
printf 'Case C: control, no mask\n'
ws="$root/dir-ws"
out="$("$bwrap" "${common[@]}" \
  --bind "$ws" /tmp/src --chdir /tmp/src -- \
  sh -c 'cat .git/HEAD 2>/dev/null || printf "head=denied\n"' 2>&1)"
printf '%s\n' "$out" | sed 's/^/  /'
check "the control leaks" "SECRET" "$(printf '%s\n' "$out" | head -1)"
printf '\n'

if [ "$fail" -ne 0 ]; then
  printf 'RESULT: at least one check failed\n'
  exit 1
fi
printf 'RESULT: every check passed\n'
