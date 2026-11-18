#!/usr/bin/env bash
# T-001: can a jj repository be backed by a bare Git repository and host
# multiple jj workspaces, one per sandbox?
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
work="$here/work"
rm -rf "$work"
mkdir -p "$work"

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
echo '--- create the jj bridge backed by the bare Git repo ---'
jj git init --no-colocate --git-repo="$bare" "$work/bridge"

cd "$work/bridge"
echo '--- bridge workspace list ---'
jj workspace list

echo '--- add two sandbox workspaces ---'
jj workspace add "$work/ws1" --name sandbox-1
jj workspace add "$work/ws2" --name sandbox-2
jj workspace list

echo '--- change in ws1 ---'
cd "$work/ws1"
printf 'one\n' > one.txt
jj commit -m 'ws1 change'

echo '--- change in ws2 ---'
cd "$work/ws2"
printf 'two\n' > two.txt
jj commit -m 'ws2 change'

echo '--- jj log from the bridge ---'
cd "$work/bridge"
jj log --no-pager

echo '--- bare Git refs before bookmarks and export ---'
git --git-dir="$bare" for-each-ref

echo '--- create bookmarks for the sandbox changes ---'
cd "$work/ws1"
jj bookmark create litterbox/sandbox-1 -r @-
cd "$work/ws2"
jj bookmark create litterbox/sandbox-2 -r @-

echo '--- jj git export ---'
cd "$work/bridge"
jj git export

echo '--- bare Git refs after bookmarks and export ---'
git --git-dir="$bare" for-each-ref

echo '--- bridge directory ---'
ls -a "$work/bridge"
