#!/usr/bin/env bash
# T-023 probe B — the two jj workflows.
#
# Flow 1: Litterbox introduces the conflict (the divergence merge). Resolve in
#         place in the sandbox workspace and advance the bookmark.
# Flow 2: the user introduced the conflict (a conflicted shared-branch tip).
#         Resolve in a child and squash it into the introducing commit.
set -euo pipefail

root="$(cd "$(dirname "$0")" && pwd)/jj-workflow"
rm -rf "$root"
mkdir -p "$root/repo"
cd "$root/repo"

jj git init --colocate . >/dev/null
jj config set --repo user.name probe
jj config set --repo user.email probe@example.com

seq 24 | sed 's/^/line /' > file.txt
printf 'other\n' > other.txt
jj describe -m base >/dev/null
base="$(jj log --no-graph -r @ -T 'commit_id')"
jj bookmark create litterbox/probe -r "$base" >/dev/null

rewrite() { sed -i "1s/.*/$2 line 1/;20s/.*/$2 line 20/" "$1"; }
resolve_awk() {
  awk '
    /^<<<<<<</ { print "resolved: both sides wanted this line"; next }
    /^=======/  { next }
    /^>>>>>>>/  { next }
    { print }
  ' "$1" > "$1.new"
  mv "$1.new" "$1"
}

jj new "$base" -m "theirs: user rewrote the file" >/dev/null
rewrite file.txt theirs
theirs="$(jj log --no-graph -r @ -T 'commit_id')"

jj new "$base" -m "ours: sandbox rewrote the file" >/dev/null
rewrite file.txt ours
ours="$(jj log --no-graph -r @ -T 'commit_id')"

echo "== flow 1: Litterbox introduces the merge"
jj new "$theirs" "$ours" -m "merge the divergence" >/dev/null
merge="$(jj log --no-graph -r @ -T 'commit_id')"
echo "-- conflicts"; jj resolve --list -r "$merge" || true
echo "-- marker regions in the working copy: $(grep -c '^<<<<<<<' file.txt || true)"
echo "-- conflicts() matches: $(jj log --no-graph -r 'conflicts()' -T 'commit_id.short() ++ " " ++ description.first_line() ++ "\n"')"

echo "-- resolve in place"
resolve_awk file.txt
echo "-- conflicts on the captured id after the edit"; jj resolve --list -r "$merge" || true
echo "-- marker regions: $(grep -c '^<<<<<<<' file.txt || true)"
echo "-- the commit id the working copy holds now: $(jj log --no-graph -r @ -T 'commit_id')"
echo "   (was $merge before the edit)"
echo "-- conflicts on the rewritten commit: [$(jj resolve --list -r @ || true)]"
echo "-- conflicts() after resolving: [$(jj log --no-graph -r 'conflicts()' -T 'commit_id.short() ++ " "')]"
jj bookmark set --allow-backwards litterbox/probe -r @ >/dev/null
echo "-- bookmark: $(jj log --no-graph -r 'litterbox/probe' -T 'commit_id.short()')"
echo "-- sandbox commit still present: $(jj log --no-graph -r "$ours" -T 'commit_id.short()')"

echo
echo "== flow 2: the user's shared-branch tip is itself conflicted"
jj new "$theirs" "$ours" -m "user merge, committed without resolving" >/dev/null
tip="$(jj log --no-graph -r @ -T 'commit_id')"
jj bookmark set --allow-backwards litterbox/probe -r "$tip" >/dev/null
echo "-- tip conflicts"; jj resolve --list -r "$tip" || true
echo "-- the inherited conflict reaches a clean child"
jj new "$tip" -m "resolve the user's merge" >/dev/null
child="$(jj log --no-graph -r @ -T 'commit_id')"
echo "   conflicts on the child: $(jj resolve --list -r "$child" | wc -l) path(s)"
echo "-- resolve in the child"
resolve_awk file.txt
echo "   conflicts after the edit: [$(jj resolve --list -r @ || true)]"
echo "-- squash into the introducing commit"
set +e
squash_out="$(jj squash --use-destination-message 2>&1)"
squash_status=$?
set -e
echo "   exit: $squash_status"
printf '%s\n' "$squash_out" | sed 's/^/   /'
newtip="$(jj log --no-graph -r 'litterbox/probe' -T 'commit_id')"
echo "-- bookmark now: ${newtip:0:12}"
echo "-- conflicts on the rewritten introducing commit: [$(jj resolve --list -r "$newtip" || true)]"
echo "-- conflicts() anywhere: [$(jj log --no-graph -r 'conflicts()' -T 'commit_id.short() ++ " "')]"
echo "-- marker regions in the working copy: $(grep -c '^<<<<<<<' file.txt || true)"
echo "-- sandbox commit still present: $(jj log --no-graph -r "$ours" -T 'commit_id.short()')"
echo "-- graph"
jj log --no-graph -r '::litterbox/probe' -T 'commit_id.short() ++ " " ++ description.first_line() ++ "\n"' 2>/dev/null | head -8
