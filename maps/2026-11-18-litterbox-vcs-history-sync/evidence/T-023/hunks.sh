#!/usr/bin/env bash
# T-023 probe: can each backend report conflicted paths with a hunk count and a
# side count, without shipping the hunks themselves?
#
# Two hunks per file on purpose: each side rewrites line 1 and line 20.
set -euo pipefail

root="$(cd "$(dirname "$0")" && pwd)/hunks"
rm -rf "$root"
mkdir -p "$root"

seq_file() {
  local n=0
  while [ "$n" -lt 24 ]; do printf 'line %02d\n' "$n"; n=$((n + 1)); done
}

rewrite() {
  # $1 file, $2 side label
  sed -i "1s/.*/$2 line 00/;20s/.*/$2 line 19/" "$1"
}

echo '== jj =='
jj git init --colocate "$root/jj" >/dev/null 2>&1
cd "$root/jj"
jj config set --repo user.name probe
jj config set --repo user.email probe@example.com
seq_file > file.txt
printf 'other\n' > other.txt
jj describe -m base >/dev/null 2>&1
base="$(jj log --no-graph -r @ -T 'commit_id')"
jj bookmark create litterbox/probe -r "$base" >/dev/null

jj new "$base" -m "theirs: rewrite from the user side" >/dev/null 2>&1
rewrite file.txt theirs
theirs="$(jj log --no-graph -r @ -T 'commit_id')"

jj new "$base" -m "ours: rewrite from the sandbox side" >/dev/null 2>&1
rewrite file.txt ours
ours="$(jj log --no-graph -r @ -T 'commit_id')"

jj new "$theirs" "$ours" -m "merge" >/dev/null 2>&1
merge="$(jj log --no-graph -r @ -T 'commit_id')"

echo "-- jj resolve --list on the merge"
jj resolve --list -r "$merge" || true
echo "-- conflict template"
jj log --no-graph -r "$merge" -T 'if(conflict, "conflicted", "clean")' ; echo
echo "-- ancestors carrying conflicts"
jj log --no-graph -r "::$merge & conflicts()" -T 'commit_id.short() ++ " " ++ description.first_line() ++ "\n"' || true
echo "-- marker regions in the materialised tree"
grep -c '^<<<<<<<' file.txt || true
echo "-- sides per path from the marker file"
printf 'left/right pairs: '; grep -c '^<<<<<<<' file.txt || true
echo "-- jj diff --git hunk headers for the conflict"
jj diff -r "$merge" --git 2>/dev/null | grep -c '^@@' || true
echo "-- does a conflict in an ancestor survive a clean descendant?"
jj new "$merge" -m "child of the conflicted merge" >/dev/null 2>&1
jj resolve --list -r @ || true

echo
echo '== git =='
git init -q "$root/git"
cd "$root/git"
git config user.name probe
git config user.email probe@example.com
seq_file > file.txt
printf 'other\n' > other.txt
git add -A && git commit -qm base
base="$(git rev-parse HEAD)"
git checkout -q -b litterbox/probe

git checkout -q -b theirs "$base"
rewrite file.txt theirs
git commit -qam "theirs: rewrite from the user side"
theirs="$(git rev-parse HEAD)"

git checkout -q -b ours "$base"
rewrite file.txt ours
git commit -qam "ours: rewrite from the sandbox side"
ours="$(git rev-parse HEAD)"

echo "-- merge-tree with the recorded base"
set +e
tree="$(git merge-tree --write-tree --merge-base="$base" "$theirs" "$ours")"
status=$?
set -e
echo "exit: $status"
echo "tree: $(printf '%s' "$tree" | head -1)"

echo "-- conflicted paths from merge-tree output"
printf '%s\n' "$tree" | tail -n +2

echo "-- index stages after a real merge (unmerged paths and their stages)"
# The staging shape: a branch at the shared tip, merging the sandbox branch.
git checkout -q -B litterbox/probe "$theirs"
git merge --no-commit --no-ff "$ours" >/dev/null 2>&1 || true
git ls-files -u | awk '{print $3, $4}' | sort -u
echo "-- stage count per path"
git ls-files -u | awk '{print $4}' | sort | uniq -c
echo "-- MERGE_HEAD present"
test -f .git/MERGE_HEAD && echo yes || echo no
echo "-- marker regions in the worktree"
grep -c '^<<<<<<<' file.txt || true
echo "-- marker regions in the conflicted tree, via archive"
git archive "$(printf '%s' "$tree" | head -1)" | tar -xO file.txt | grep -c '^<<<<<<<' || true
git merge --abort 2>/dev/null || true
