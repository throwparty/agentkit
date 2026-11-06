#!/usr/bin/env bash
# T-024: does a bwrap holder survive the short-lived process that created it?
#
# Launches a holder from a bash -c that exits immediately, then joins it from a
# later process. Run twice: with and without --die-with-parent.
#
#   bash maps/2026-10-31-nix-devshell-sandbox/evidence/T-024-holder-survival.sh
#
# Requires nix, bubblewrap (built from nixpkgs#bubblewrap), nsenter and an
# unprivileged user namespace. Recorded on NixOS, kernel 7.2, bwrap 0.12.0.
set -u
BWRAP=$(nix build --no-link --print-out-paths nixpkgs#bubblewrap 2>/dev/null)/bin/bwrap
SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/litterbox-t024.XXXXXX")

# Terminate only the holder this probe launched: the outer bwrap process, and
# the payload PID 1 inside its own PID namespace. Both are checked against what
# was recorded so a recycled PID is not signalled.
kill_holder() {
  local root=$1
  local outer="" child=""
  [[ -s $root/launched.pid ]] && outer=$(<"$root/launched.pid")
  [[ -s $root/status ]] && \
    child=$(grep -o '"child-pid": [0-9]*' "$root/status" | grep -o '[0-9]*' || true)

  if [[ -n $child && -s $root/child.pidns ]] \
    && [[ $(readlink "/proc/$child/ns/pid" 2>/dev/null) == $(<"$root/child.pidns") ]]; then
    kill -KILL "$child" 2>/dev/null || true
  fi
  if [[ -n $outer ]] \
    && [[ $(basename "$(readlink "/proc/$outer/exe" 2>/dev/null)") == bwrap ]]; then
    kill "$outer" 2>/dev/null || true
  fi
}

cleanup() {
  local root
  for root in "$SCRATCH"/holder.*; do
    [[ -d $root ]] && kill_holder "$root"
  done
  rm -rf "$SCRATCH"
}
trap cleanup EXIT

echo "bwrap: $BWRAP"
echo "bwrap version: $($BWRAP --version)"

probe() {
  local name=$1; shift
  local root status
  root=$(mktemp -d "$SCRATCH/holder.XXXXXX")
  status=$root/status
  echo
  echo "=== $name"
  bash -c "
    exec 3>\"$status\"
    \"$BWRAP\" --unshare-user --unshare-pid --unshare-ipc --unshare-uts --unshare-cgroup --unshare-net \
      $* \
      --clearenv --setenv PATH /run/current-system/sw/bin \
      --ro-bind /nix/store /nix/store \
      --ro-bind /run/current-system /run/current-system \
      --ro-bind /etc /etc \
      --proc /proc --dev /dev --tmpfs /tmp \
      --bind \"$root\" /src --chdir /src \
      --json-status-fd 3 -- sleep 300 &
    echo \$! > \"$root/launched.pid\"
  "
  echo "launcher exited: $?"
  sleep 1
  echo "holder processes still alive after launcher exit:"
  pgrep -af "sleep 300" || echo "  (none)"
  if [[ -s $status ]]; then
    local child
    child=$(grep -o '"child-pid": [0-9]*' "$status" | grep -o '[0-9]*')
    echo "child-pid from status: $child"
    readlink "/proc/$child/ns/pid" > "$root/child.pidns" 2>/dev/null || true
    nsenter -t "$child" -U -i -n -p -m -u --preserve-credentials \
      -r"/proc/$child/root" -W/src -- /run/current-system/sw/bin/id 2>&1 \
      | sed 's/^/  join: /'
  else
    echo "no status document written"
  fi
  kill_holder "$root"
  sleep 0.3
}

probe "with --die-with-parent" --die-with-parent
probe "without --die-with-parent" ""
