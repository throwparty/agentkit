#!/usr/bin/env bash
# Mask a bound workspace's VCS metadata with a Docker empty volume.
#
# T-002 recorded that an empty volume at `/workspace/.jj` hides the contents.
# This probe answers the two remaining questions from the hide-vcs ADR:
#
#   1. Does Docker create the mountpoint when the bound directory holds no
#      `.git` at all, so a path can be masked before it exists?
#   2. What check establishes that the daemon shares the host filesystem,
#      which `Bound` needs because the bind source is resolved on the
#      daemon's side?
#
# Run from the repository root:
#   bash adrs/2026-11-21-hide-vcs-metadata-from-sandboxes/evidence/T-015/probe.sh

set -uo pipefail

image="${IMAGE:-busybox:latest}"
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

printf 'docker: %s\n' "$(docker version --format '{{.Server.Version}}')"
printf 'root: %s\n' "$(docker info --format '{{.DockerRootDir}}')"
printf 'image: %s\n' "$image"
printf 'scratch: %s\n\n' "$root"

# Case A: `.git` exists and is masked with an empty volume.
printf 'Case A: .git exists, masked with an empty volume\n'
ws="$root/dir-ws"
mkdir -p "$ws/.git"
printf 'SECRET\n' >"$ws/.git/HEAD"
printf 'hello\n' >"$ws/README.md"
mkdir -p "$root/empty-a"

out="$(docker run --rm --network none \
  -v "$ws":/workspace -v "$root/empty-a":/workspace/.git \
  "$image" sh -c '
    printf "readme=%s\n" "$(cat /workspace/README.md)"
    printf "entries=%s\n" "$(ls -A /workspace/.git 2>/dev/null | wc -l)"
    if printf x > /workspace/.git/written 2>/dev/null; then printf "write=volume\n"; else printf "write=denied\n"; fi
  ' 2>&1)"
printf '%s\n' "$out" | sed 's/^/  /'
check "readme is readable" "hello" "$(field "$out" readme)"
check "the entry is empty" "0" "$(field "$out" entries)"
check "the host .git is untouched" "SECRET" "$(cat "$ws/.git/HEAD")"
check "no file lands in the host .git" "absent" \
  "$([ -e "$ws/.git/written" ] && echo present || echo absent)"
printf '\n'

# Case B: the bound directory holds no `.git`; the mask names a path that
# does not exist.
printf 'Case B: .git is absent, masked with a bind of an empty host directory\n'
ws="$root/absent-ws"
mkdir -p "$ws"
mkdir -p "$root/empty-b"
printf 'hello\n' >"$ws/README.md"

out="$(docker run --rm --network none \
  -v "$ws":/workspace -v "$root/empty-b":/workspace/.git \
  "$image" sh -c '
    printf "readme=%s\n" "$(cat /workspace/README.md)"
    if [ -d /workspace/.git ]; then printf "entry=present\n"; else printf "entry=absent\n"; fi
    printf "entries=%s\n" "$(ls -A /workspace/.git 2>/dev/null | wc -l)"
    if printf x > /workspace/.git/written 2>/dev/null; then printf "write=accepted\n"; else printf "write=denied\n"; fi
  ' 2>&1)"
printf '%s\n' "$out" | sed 's/^/  /'
check "readme is readable" "hello" "$(field "$out" readme)"
check "the mountpoint is a directory" "present" "$(field "$out" entry)"
check "the mountpoint is empty" "0" "$(field "$out" entries)"
printf '  NOTE: the host workspace %s a .git entry after the bind\n' \
  "$([ -e "$ws/.git" ] && echo has || echo lacks)"
printf '\n'

# Case B2: the same, with a named volume rather than a host directory.
printf 'Case B2: .git is absent, masked with a named volume\n'
ws="$root/absent-v-ws"
mkdir -p "$ws"
printf 'hello\n' >"$ws/README.md"
docker volume create litterbox-probe-mask >/dev/null 2>&1

out="$(docker run --rm --network none \
  -v "$ws":/workspace -v litterbox-probe-mask:/workspace/.git \
  "$image" sh -c '
    printf "readme=%s\n" "$(cat /workspace/README.md)"
    if [ -d /workspace/.git ]; then printf "entry=present\n"; else printf "entry=absent\n"; fi
    printf "entries=%s\n" "$(ls -A /workspace/.git 2>/dev/null | wc -l)"
  ' 2>&1)"
printf '%s\n' "$out" | sed 's/^/  /'
check "the mountpoint is a directory" "present" "$(field "$out" entry)"
check "the mountpoint is empty" "0" "$(field "$out" entries)"
printf '  NOTE: the host workspace %s a .git entry after the bind\n' \
  "$([ -e "$ws/.git" ] && echo has || echo lacks)"
docker volume rm litterbox-probe-mask >/dev/null 2>&1
printf '\n'

# Case C: the same-filesystem check. A sentinel written on the host and read
# through a bind is visible only when the daemon shares the host filesystem.
printf 'Case C: does the daemon share the host filesystem?\n'
sentinel="$root/sentinel"
printf 'host-side\n' >"$sentinel"
out="$(docker run --rm --network none \
  -v "$sentinel":/sentinel:ro \
  "$image" sh -c 'cat /sentinel 2>/dev/null || printf "sentinel=unreadable\n"' 2>&1)"
printf '%s\n' "$out" | sed 's/^/  /'
if [ "$(printf '%s\n' "$out" | head -1)" = "host-side" ]; then
  printf '  daemon: shares the host filesystem\n'
else
  printf '  daemon: does not share the host filesystem\n'
fi
printf '\n'

# Case D: the control. Without a mask the same workspace leaks.
printf 'Case D: control, no mask\n'
ws="$root/dir-ws"
out="$(docker run --rm --network none \
  -v "$ws":/workspace "$image" sh -c 'cat /workspace/.git/HEAD 2>/dev/null || printf "head=denied\n"' 2>&1)"
printf '%s\n' "$out" | sed 's/^/  /'
check "the control leaks" "SECRET" "$(printf '%s\n' "$out" | head -1)"
printf '\n'

if [ "$fail" -ne 0 ]; then
  printf 'RESULT: at least one check failed\n'
  exit 1
fi
printf 'RESULT: every check passed\n'
