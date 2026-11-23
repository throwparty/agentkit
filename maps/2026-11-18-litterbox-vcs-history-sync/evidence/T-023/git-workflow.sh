#!/usr/bin/env bash
# T-023 probe A — can a resolved marker tree in /src (no .git, no index) become
# a host-written Git merge commit whose tip is clean?
#
# /src is simulated as a plain directory holding only files: the tree Git's
# merge produced, materialised with no index and no metadata.
set -euo pipefail

root="$(cd "$(dirname "$0")" && pwd)/git-workflow"
rm -rf "$root"
mkdir -p "$root/repo"
cd "$root/repo"

git init -q .
git config user.name probe
git config user.email probe@example.com

seq 24 | sed 's/^/line /' > file.txt
printf 'other\n' > other.txt
git add -A && git commit -qm base
base="$(git rev-parse HEAD)"

rewrite() { sed -i "1s/.*/$2 line 1/;20s/.*/$2 line 20/" "$1"; }

git checkout -q -b theirs "$base"
rewrite file.txt theirs
git commit -qam "theirs: user rewrote the file"
theirs="$(git rev-parse HEAD)"

git checkout -q -b ours "$base"
rewrite file.txt ours
git commit -qam "ours: sandbox rewrote the file"
ours="$(git rev-parse HEAD)"

echo "== the merge Litterbox computes"
set +e
merged="$(git merge-tree --write-tree --merge-base="$base" "$theirs" "$ours")"
status=$?
set -e
tree="$(printf '%s\n' "$merged" | head -1)"
echo "merge-tree exit: $status"
echo "conflict tree:   $tree"

echo
echo "== materialise the conflict tree into /src (files only)"
mkdir -p "$root/src"
git archive "$tree" | tar -x -C "$root/src"
echo "entries in /src: $(ls -A "$root/src" | tr '\n' ' ')"
echo "has .git:        $([ -e "$root/src/.git" ] && echo yes || echo no)"
echo "marker regions:  $(grep -c '^<<<<<<<' "$root/src/file.txt")"

echo
echo "== the agent edits the markers away (no VCS involved)"
awk '
/^<<<<<<</ { print "resolved: both sides wanted this line"; next }
/^=======/  { next }
/^>>>>>>>/  { next }
{ print }
' "$root/src/file.txt" > "$root/src/file.txt.new"
mv "$root/src/file.txt.new" "$root/src/file.txt"
echo "marker regions after edit: $(grep -c '^<<<<<<<' "$root/src/file.txt" || true)"

echo
echo "== the host turns the resolved directory into a merge commit"
export GIT_INDEX_FILE="$root/resolve.index"
rm -f "$GIT_INDEX_FILE"
git --work-tree="$root/src" add -A
resolved_tree="$(git write-tree)"
unset GIT_INDEX_FILE
commit="$(git commit-tree "$resolved_tree" -p "$theirs" -p "$ours" -m "resolve the divergence")"

echo "resolved tree: $resolved_tree"
echo "merge commit:  $commit"
echo "parents:       $(git rev-list --parents -n 1 "$commit" | cut -d' ' -f2-)"

echo
echo "== move the shared branch and check the result"
git checkout -q -B litterbox/probe "$commit"
echo "-- status"; git status --short; echo "(empty means clean)"
echo "-- markers in the tip tree"
git archive HEAD | tar -xO file.txt | grep -c '^<<<<<<<' || echo 0
echo "-- graph"
git log --graph --oneline --decorate -4
echo "-- is the sandbox tip contained?"
git merge-base --is-ancestor "$ours" HEAD && echo "yes: ours is an ancestor"
echo "-- does a plain merge of the two sides now conflict?"
git merge-tree --write-tree --merge-base="$base" "$theirs" HEAD >/dev/null 2>&1 \
  && echo "no conflict" || echo "still conflicts"
