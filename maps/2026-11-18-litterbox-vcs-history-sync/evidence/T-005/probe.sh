#!/usr/bin/env bash
# T-005: can a conflicted Git commit be marked with refs/notes so Litterbox can
# compute inherited conflict for descendants, and what is the layout?
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

printf 'apple\ngrapefruit\norange\n' > file.txt
git commit -qam ours
ours=$(git rev-parse HEAD)

git checkout -q "$base"
printf 'APPLE\nGRAPE\nORANGE\n' > file.txt
git commit -qam theirs
theirs=$(git rev-parse HEAD)

set +e
merge_output=$(git merge-tree --write-tree --merge-base="$base" "$ours" "$theirs" 2>&1)
set -e
tree=$(printf '%s\n' "$merge_output" | head -n1)
conflicted=$(git commit-tree "$tree" -p "$ours" -p "$theirs" -m 'conflicted merge')

notes_ref='refs/notes/litterbox/conflicts'
git notes --ref="$notes_ref" add -m "state=conflicted merge=1 base=$base ours=$ours theirs=$theirs" "$conflicted"

printf 'conflicted=%s\n' "$conflicted"
printf '%s\n' '--- note on the conflicted commit ---'
git notes --ref="$notes_ref" show "$conflicted"
printf '%s\n' '--- notes list ---'
git notes --ref="$notes_ref" list

printf '%s\n' '--- a descendant of the conflicted commit ---'
git checkout -q "$conflicted"
printf 'child\n' > child.txt
git add child.txt
git commit -qm 'child on top of the conflict'
child=$(git rev-parse HEAD)
printf 'child=%s\n' "$child"

printf '%s\n' '--- grandchild ---'
printf 'grandchild\n' > grandchild.txt
git add grandchild.txt
git commit -qm 'grandchild'
grandchild=$(git rev-parse HEAD)
printf 'grandchild=%s\n' "$grandchild"

printf '%s\n' '--- walk ancestors of the grandchild and check for the note ---'
for rev in $(git rev-list "$grandchild"); do
  if git notes --ref="$notes_ref" show "$rev" >/dev/null 2>&1; then
    printf 'conflicted ancestor: %s\n' "$rev"
    git notes --ref="$notes_ref" show "$rev"
  fi
done

printf '%s\n' '--- normal clone does not fetch the notes ref ---'
cd "$work"
git clone -q repo clone
cd clone
git notes --ref="$notes_ref" list 2>&1 || true
printf '%s\n' '--- explicit notes refspec fetch ---'
git fetch -q origin "+refs/notes/*:refs/notes/*"
git notes --ref="$notes_ref" show "$conflicted"
