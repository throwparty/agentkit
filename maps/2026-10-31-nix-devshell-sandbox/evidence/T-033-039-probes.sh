#!/usr/bin/env bash
# Probes for T-033 to T-039: the facts the linux-bwrap-provider spec rests on.
#
# Run on a Linux host with bubblewrap and nix:
#
#   bash maps/2026-10-31-nix-devshell-sandbox/evidence/T-033-039-probes.sh
#
# The recorded output is in the evidence files beside this script. The script
# starts real sandboxes and kills them at the end. Nothing is written outside a
# temporary directory.
set -uo pipefail

BWRAP=${BWRAP:-$(command -v bwrap || true)}
if [[ -z ${BWRAP} ]] && command -v nix >/dev/null 2>&1; then
  BWRAP="$(nix build --no-link --print-out-paths nixpkgs#bubblewrap 2>/dev/null)/bin/bwrap"
fi

WORK=$(mktemp -d /tmp/bwrap-probes.XXXXXX)
ROOT="$WORK/root"
mkdir -p "$ROOT"
STATUS="$WORK/status.json"
HOLD=2147483647
cd /

if [[ -z ${BWRAP} || ! -x $BWRAP ]]; then
  echo "no bwrap; set BWRAP to a bubblewrap binary" >&2
  exit 1
fi

start_holder() {
  local status=$1
  shift
  : >"$status"
  local t0
  t0=$(date +%s%N)
  "$BWRAP" \
    --unshare-user --unshare-pid --unshare-ipc --unshare-uts --unshare-cgroup --unshare-net \
    --disable-userns \
    --clearenv --setenv PATH /run/current-system/sw/bin \
    "$@" \
    --ro-bind /run/current-system /run/current-system \
    --ro-bind /etc /etc \
    --proc /proc --dev /dev --tmpfs /tmp \
    --dir /src --bind "$ROOT" /src --chdir /src \
    --json-status-fd 3 \
    -- sleep "$HOLD" 3>"$status" &
  LAUNCHER=$!
  for _ in $(seq 1 1200); do
    [[ -s $status ]] && break
    sleep 0.01
  done
  STATUS_MS=$((($(date +%s%N) - t0) / 1000000))
  PID=$(grep -o '"child-pid": *[0-9]*' "$status" | grep -o '[0-9]*')
  # The status document arrives long before the view is built, so wait until
  # bwrap has moved the old root away and the workspace is mounted.
  for _ in $(seq 1 1200); do
    if ! grep -q ' /oldroot ' "/proc/$PID/mountinfo" 2>/dev/null &&
      grep -q " $ROOT \| /src " "/proc/$PID/mountinfo" 2>/dev/null; then
      break
    fi
    sleep 0.01
  done
  READY_MS=$((($(date +%s%N) - t0) / 1000000))
}

# Join a command. setns into a pid namespace only moves later children, and
# nsenter forks for that reason, which is why this is a fair stand-in for the
# in-process join the backend will do.
enter() {
  nsenter -t "$PID" -U -i -n -p -m -u --preserve-credentials -r"/proc/$PID/root" -- "$@"
}

stop_holder() {
  kill -9 "$PID" 2>/dev/null
  wait "$LAUNCHER" 2>/dev/null
}

echo "=== environment"
echo "bwrap: $("$BWRAP" --version)"
echo "kernel: $(uname -r)"
echo "cgroup fs: $(stat -fc %T /sys/fs/cgroup)"
if unshare -U true 2>/dev/null; then
  echo "unprivileged user namespaces: permitted"
else
  echo "unprivileged user namespaces: refused"
fi
if systemctl --user show-environment >/dev/null 2>&1; then
  echo "user manager: present"
else
  echo "user manager: absent"
fi
if systemd-run --user --scope --unit="bwrap-probe-$$" --quiet true >/dev/null 2>&1; then
  echo "user scope: starts"
else
  echo "user scope: does not start"
fi

echo
echo "=== T-033 and T-039: the holder command and when the sandbox is usable"
start_holder "$STATUS" --ro-bind /nix/store /nix/store
echo "status: $(tr -d '\n' <"$STATUS")"
echo "status document after: ${STATUS_MS}ms"
echo "view usable after: ${READY_MS}ms"
if kill -0 "$PID" 2>/dev/null; then
  echo "holder alive: yes"
  echo "holder cmdline: $(tr '\0' ' ' <"/proc/$PID/cmdline")"
else
  echo "holder alive: no"
fi

echo
echo "=== T-036: namespace inodes across commands"
before=$(for ns in user ipc net pid mnt uts cgroup; do readlink "/proc/$PID/ns/$ns"; done)
for i in 1 2 3; do enter /run/current-system/sw/bin/true >/dev/null 2>&1; done
after=$(for ns in user ipc net pid mnt uts cgroup; do readlink "/proc/$PID/ns/$ns"; done)
echo "$before" | tr '\n' ' '
echo
if [[ $before == "$after" ]]; then
  echo "inodes unchanged after 3 commands: yes"
else
  echo "inodes unchanged after 3 commands: no"
fi

echo
echo "=== T-038: setns into the cgroup namespace"
holder_cgroup=$(readlink "/proc/$PID/ns/cgroup")
joined_cgroup=$(nsenter -t "$PID" -U -C --preserve-credentials -- readlink /proc/self/ns/cgroup 2>&1)
echo "holder: $holder_cgroup"
echo "joined: $joined_cgroup"
if [[ $holder_cgroup == "$joined_cgroup" ]]; then
  echo "cgroup namespace joined: yes"
else
  echo "cgroup namespace joined: no"
fi

echo
echo "=== T-034: a client killed while its command runs"
enter /run/current-system/sw/bin/sh -c 'sleep 2' &
client=$!
sleep 0.3
kill -9 "$client" 2>/dev/null
wait "$client" 2>/dev/null
sleep 0.5
echo "process table 0.5s after the client died:"
enter /run/current-system/sw/bin/ps -o pid,ppid,stat,comm -e 2>&1 | head -10
sleep 4
echo "process table 4.5s after the client died:"
enter /run/current-system/sw/bin/ps -o pid,ppid,stat,comm -e 2>&1 | head -10
defunct=$(enter /run/current-system/sw/bin/sh -c 'for p in /proc/[0-9]*; do s=$(cut -d" " -f3 "$p/stat" 2>/dev/null); [ "$s" = Z ] && basename "$p"; done | wc -l' 2>/dev/null)
echo "defunct processes: ${defunct:-error}"
stop_holder

echo
echo "=== T-035: a closure larger than 238 paths"
mapfile -t CLOSURE < <(nix path-info -r /run/current-system 2>/dev/null | sort -u)
echo "closure paths: ${#CLOSURE[@]}"
args=()
for p in "${CLOSURE[@]}"; do args+=(--ro-bind "$p" "$p"); done
CSTATUS="$WORK/status-large.json"
start_holder "$CSTATUS" "${args[@]}"
echo "status: $(tr -d '\n' <"$CSTATUS")"
echo "status document after: ${STATUS_MS}ms"
echo "view usable after: ${READY_MS}ms"
if [[ -n $PID ]] && kill -0 "$PID" 2>/dev/null; then
  echo "holder alive: yes"
  echo "mountinfo lines: $(wc -l <"/proc/$PID/mountinfo")"
  echo "store mounts: $(awk '{print $5}' "/proc/$PID/mountinfo" | grep -c '^/nix/store/' || true)"
  echo -n "command inside: "
  enter /run/current-system/sw/bin/sh -c 'echo ok' 2>&1
  stop_holder
else
  echo "holder alive: no"
fi

rm -rf "$WORK"
