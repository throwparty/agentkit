#!/usr/bin/env bash
# T-004: does `git merge-tree --write-tree --merge-base` produce a usable
# conflict tree, and can that tree be committed as a conflicted commit?
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
work="$here/work"
rm -rf "$work"
mkdir -p "$work"
cd "$work"

git init -q
git config user.name probe
git config user.email probe@example.com

printf 'apple\ngrape\norange\n' > file.txt
git add file.txt
git commit -qm base
base=$(git rev-parse HEAD)

printf 'apple\ngrapefruit\norange\n' > file.txt
git commit -qam ours
ours=$(git rev-parse HEAD)

git checkout -q "$base"
printf 'APPLE\nGRAPE\nORANGE\n' > file.txt
git commit -qam theirs
theirs=$(git rev-parse HEAD)

set +e
merge_output=$(git merge-tree --write-tree --merge-base="$base" "$ours" "$theirs" 2>&1)
status=$?
set -e
tree=$(printf '%s\n' "$merge_output" | head -n1)

printf 'merge-tree status=%s\n' "$status"
printf 'merge-tree tree=%s\n' "$tree"
printf '%s\n' '--- merge-tree output ---'
printf '%s\n' "$merge_output"
printf '%s\n' '--- tree ---'
git cat-file -p "$tree"
printf '%s\n' '--- file.txt from the tree ---'
git show "$tree:file.txt"

commit=$(git commit-tree "$tree" -p "$ours" -p "$theirs" -m 'conflicted merge')
printf 'commit-tree commit=%s\n' "$commit"
printf '%s\n' '--- commit ---'
git cat-file -p "$commit"
printf '%s\n' '--- log ---'
git log --oneline --graph --all
