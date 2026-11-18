#!/usr/bin/env bash
# T-003: how jj exposes conflict paths and sides, and whether editing the
# materialised markers resolves the conflict or `jj resolve` is required.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
work="$here/work"
rm -rf "$work"
mkdir -p "$work"
cd "$work"

git init -q repo
cd repo
git config user.name probe
git config user.email probe@example.com

printf 'apple\ngrape\norange\n' > file.txt
git add file.txt
git commit -qm base
base=$(git rev-parse HEAD)

git checkout -q -b ours
printf 'apple\ngrapefruit\norange\n' > file.txt
git commit -qam ours

git checkout -q "$base"
git checkout -q -b theirs
printf 'APPLE\nGRAPE\nORANGE\n' > file.txt
git commit -qam theirs

jj git init --colocate >/dev/null
echo '--- bookmarks ---'
jj bookmark list
echo '--- merge ours and theirs ---'
jj new ours theirs -m merge
echo '--- status after the merge ---'
jj status
echo '--- log after the merge ---'
jj log --no-pager
echo '--- resolve --list ---'
jj resolve --list || true
echo '--- materialised conflict ---'
cat file.txt
echo '--- sides: base -> ours ---'
jj diff --from "$base" --to ours --git -- file.txt || true
echo '--- sides: base -> theirs ---'
jj diff --from "$base" --to theirs --git -- file.txt || true

echo '--- edit the marker file and snapshot ---'
printf 'APPLE\nGRAPEFRUIT\nORANGE\n' > file.txt
jj status
echo '--- log after editing ---'
jj log --no-pager
echo '--- resolve --list after editing ---'
jj resolve --list || true

echo '--- squash the resolution into the merge commit ---'
jj squash -m merge --use-destination-message || true
jj status
echo '--- log after squash ---'
jj log --no-pager
echo '--- resolve --list after squash ---'
jj resolve --list || true
