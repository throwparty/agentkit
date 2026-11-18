#!/usr/bin/env bash
# T-002: can a jj workspace be bind-mounted into a sandbox with `.jj` masked?
# Linux only: bubblewrap and Docker. Seatbelt is macOS-only and is recorded as
# unprobed, never assumed.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
work="$here/work"
rm -rf "$work"
mkdir -p "$work/empty"

bare="$work/bare.git"
git init -q --bare "$bare"

seed="$work/seed"
git init -q "$seed"
cd "$seed"
git config user.name probe
git config user.email probe@example.com
printf 'seed\n' > README.md
git add .
git commit -qm seed
git remote add origin "$bare"
git push -q origin HEAD:main

cd "$work"
jj git init --no-colocate --git-repo="$bare" "$work/bridge" >/dev/null
cd "$work/bridge"
jj workspace add "$work/ws" --name sandbox-1 >/dev/null

echo '=== workspace layout ==='
ls -a "$work/ws"

echo '=== bubblewrap: bind the workspace, tmpfs over .jj ==='
bwrap_bin=$(command -v bwrap || true)
if [ -z "$bwrap_bin" ]; then
  bwrap_bin=$(find /nix/store -maxdepth 3 -type f -name bwrap 2>/dev/null | head -1 || true)
fi
if [ -z "$bwrap_bin" ]; then
  echo 'bwrap not found'
else
  echo "bwrap=$bwrap_bin"
  set +e
  "$bwrap_bin" --ro-bind / / --bind "$work/ws" "$work/ws" --tmpfs "$work/ws/.jj" \
    --chdir "$work/ws" -- /bin/sh -c 'echo "top:"; ls -a; echo ".jj:"; ls -a .jj; test -z "$(ls -A .jj)" && echo EMPTY || echo NONEMPTY; test -f README.md && echo README_PRESENT || echo README_MISSING'
  status=$?
  set -e
  echo "bwrap exit=$status"
fi

echo '=== docker: bind the workspace, empty dir over .jj ==='
if ! docker info >/dev/null 2>&1; then
  echo 'docker daemon unavailable'
else
  image=$(docker images --format '{{.Repository}}:{{.Tag}}' | grep -v '<none>' | head -1 || true)
  echo "image=${image:-none}"
  if [ -z "$image" ]; then
    echo 'no local image; not pulling one automatically'
  else
    set +e
    docker run --rm -v "$work/ws:/workspace" -v "$work/empty:/workspace/.jj" \
      "$image" sh -c 'echo "top:"; ls -a /workspace; echo ".jj:"; ls -a /workspace/.jj; test -z "$(ls -A /workspace/.jj)" && echo EMPTY || echo NONEMPTY; test -f /workspace/README.md && echo README_PRESENT || echo README_MISSING'
    status=$?
    set -e
    echo "docker exit=$status"
  fi
fi

echo '=== seatbelt ==='
echo 'unprobed: macOS-only; this host is Linux'
