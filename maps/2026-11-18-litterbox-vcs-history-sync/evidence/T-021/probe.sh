#!/usr/bin/env bash
# T-021/T-022: a conflicted bridge tip is not adopted; the sandbox stays on the
# last non-conflicted revision. jj reports the conflict natively; Git is a
# marker scan (the normal Git path resolves before pushing).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
work="$here/work"
rm -rf "$work"
mkdir -p "$work"

echo '=== jj: a conflicted bridge tip vs a clean sandbox branch ==='
mkdir -p "$work/jj"
cd "$work/jj"
jj git init --colocate >/dev/null
jj config set --repo user.name probe
jj config set --repo user.email probe@example.com

printf 'apple\ngrape\norange\n' > file.txt
jj commit -m base >/dev/null
base=$(jj log --no-graph -r @- -T 'commit_id')

jj bookmark create litterbox/name -r @- >/dev/null
printf 'apple\ngrapefruit\norange\n' > file.txt
jj commit -m user >/dev/null
jj bookmark set litterbox/name -r @- >/dev/null

jj new "$base" >/dev/null
jj bookmark create litterbox/sandbox -r @ >/dev/null
printf 'APPLE\nGRAPE\nORANGE\n' > file.txt
jj commit -m sandbox >/dev/null
jj bookmark set litterbox/sandbox -r @- >/dev/null

echo '--- merge the user and sandbox branches; the user tip conflicts ---'
jj new litterbox/name litterbox/sandbox -m merge >/dev/null
jj bookmark set litterbox/name -r @ >/dev/null
jj log --no-graph -r litterbox/name -T 'if(conflict, "conflicted", "clean") ++ "\n"'
jj resolve --list -r litterbox/name || true

echo '--- move the sandbox workspace to its own branch and read it ---'
jj new litterbox/sandbox >/dev/null
cat file.txt
jj log --no-graph -r litterbox/sandbox -T 'if(conflict, "conflicted", "clean") ++ "\n"'
jj resolve --list -r litterbox/sandbox || true

echo '--- the fallback decision ---'
if jj resolve --list -r litterbox/name 2>/dev/null | grep -q .; then
  echo 'HOLD: bridge tip is conflicted; the sandbox stays on its last non-conflicted revision'
else
  echo 'ADOPT: bridge tip is clean'
fi

echo '--- resolve the bridge conflict in the working copy ---'
jj edit litterbox/name >/dev/null
printf 'APPLE\nGRAPEFRUIT\nORANGE\n' > file.txt
jj status >/dev/null
jj bookmark set litterbox/name -r @ >/dev/null
jj log --no-graph -r litterbox/name -T 'if(conflict, "conflicted", "clean") ++ "\n"'
jj resolve --list -r litterbox/name || true

echo '--- the decision after resolution ---'
if jj resolve --list -r litterbox/name 2>/dev/null | grep -q .; then
  echo 'HOLD: bridge tip still conflicted'
else
  echo 'ADOPT: bridge tip is clean; the next sandbox operation adopts it'
fi
cat file.txt

echo
echo '=== Git: a marker tree is detected before it is adopted ==='
mkdir -p "$work/git"
cd "$work/git"
git init -q
git config user.name probe
git config user.email probe@example.com
printf 'apple\n<<<<<<< ours\ngrapefruit\n=======\nGRAPE\n>>>>>>> theirs\norange\n' > file.txt
git add file.txt
git commit -qm 'unresolved markers'

echo '--- marker scan ---'
if git grep -n -E '^(<<<<<<<|=======|>>>>>>>)' HEAD -- .; then
  echo 'HOLD: the tip tree contains conflict markers'
else
  echo 'ADOPT: no conflict markers'
fi

echo '--- resolve the marker tree ---'
printf 'apple\ngrapefruit\norange\n' > file.txt
git commit -qam resolved
if git grep -n -E '^(<<<<<<<|=======|>>>>>>>)' HEAD -- .; then
  echo 'HOLD: the tip tree still contains conflict markers'
else
  echo 'ADOPT: no conflict markers; the next sandbox operation adopts it'
fi
