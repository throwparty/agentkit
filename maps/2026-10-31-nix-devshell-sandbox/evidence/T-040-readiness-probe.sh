#!/usr/bin/env bash
# Probe for T-040: does "every recorded mount is present" mean the sandbox is
# usable?
#
#   bash maps/2026-10-31-nix-devshell-sandbox/evidence/T-040-readiness-probe.sh
#
# Starts the 1779-bind view from T-035, polls the mount table until every path
# is present, and then attempts a join. Kills the holder on exit.
set -uo pipefail

BWRAP=${BWRAP:-$(command -v bwrap || true)}
if [[ -z ${BWRAP} ]] && command -v nix >/dev/null 2>&1; then
  BWRAP="$(nix build --no-link --print-out-paths nixpkgs#bubblewrap 2>/dev/null)/bin/bwrap"
fi
if [[ -z ${BWRAP} || ! -x $BWRAP ]]; then
  echo "no bwrap; set BWRAP to a bubblewrap binary" >&2
  exit 1
fi

W=$(mktemp -d /tmp/readiness.XXXXXX)
R=$W/root
mkdir -p "$R"
S=$W/status
cd /

mapfile -t C < <(nix path-info -r /run/current-system | sort -u)
args=()
for p in "${C[@]}"; do args+=(--ro-bind "$p" "$p"); done

t0=$(date +%s%N)
"$BWRAP" --unshare-user --unshare-pid --unshare-ipc --unshare-uts --unshare-cgroup --unshare-net --disable-userns \
  --clearenv --setenv PATH /run/current-system/sw/bin "${args[@]}" \
  --ro-bind /run/current-system /run/current-system --ro-bind /etc /etc \
  --proc /proc --dev /dev --tmpfs /tmp --dir /src --bind "$R" /src --chdir /src \
  --json-status-fd 3 -- sleep 2147483647 3>"$S" &
L=$!
trap 'if [[ ${P:-} =~ ^[0-9]+$ && ${P:-0} -gt 1 ]]; then kill -9 "$P" 2>/dev/null; fi
      if [[ ${L:-} =~ ^[0-9]+$ && ${L:-0} -gt 1 ]]; then kill -9 "$L" 2>/dev/null; fi
      rm -rf "$W"' EXIT

for _ in $(seq 1 600); do
  [[ -s $S ]] && break
  sleep 0.01
done
P=$(grep -o '"child-pid": *[0-9]*' "$S" | grep -o '[0-9]*')
echo "status document: $((($(date +%s%N) - t0) / 1000000))ms"
echo "expected store binds: ${#C[@]}"

all_present_at=-1
for _ in $(seq 1 4000); do
  declare -A seen=()
  while read -r mp opts; do seen["$mp"]=$opts; done < <(
    awk '{print $5, $6}' "/proc/$P/mountinfo" 2>/dev/null
  )
  missing=0
  for p in "${C[@]}"; do [[ -n ${seen[$p]:-} ]] || { missing=1; break; }; done
  if [[ $missing -eq 0 ]]; then
    all_present_at=$((($(date +%s%N) - t0) / 1000000))
    oldroot=$(grep -c ' /oldroot ' "/proc/$P/mountinfo" 2>/dev/null || true)
    echo "all ${#C[@]} recorded mounts present at: ${all_present_at}ms"
    echo "at that instant, /oldroot mounts: ${oldroot:-0}"
    echo "at that instant, /src options: ${seen[/src]:-absent}"
    break
  fi
  sleep 0.005
done
[[ $all_present_at -lt 0 ]] && echo "mounts never all present"

first_exec=-1
for i in $(seq 1 4000); do
  if nsenter -t "$P" -U -i -n -p -m -u --preserve-credentials -r"/proc/$P/root" -- \
    /run/current-system/sw/bin/true 2>/dev/null; then
    first_exec=$((($(date +%s%N) - t0) / 1000000))
    echo "first successful exec at: ${first_exec}ms (attempt $i)"
    break
  fi
  sleep 0.005
done
[[ $first_exec -lt 0 ]] && echo "exec never succeeded"
echo "gap between mounts complete and exec: $((first_exec - all_present_at))ms"
