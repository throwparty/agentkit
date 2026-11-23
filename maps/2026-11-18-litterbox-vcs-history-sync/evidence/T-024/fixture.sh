#!/usr/bin/env bash
# T-024 fixture — a jj repository whose shared bookmark points at a conflicted
# commit the "user" committed without resolving.
#
# Usage: fixture.sh <directory>
set -euo pipefail

dest="${1:?usage: fixture.sh <directory>}"
rm -rf "$dest"
mkdir -p "$dest"
cd "$dest"

jj git init --colocate . >/dev/null
jj config set --repo user.name probe >/dev/null 2>&1
jj config set --repo user.email probe@example.com >/dev/null 2>&1

seq 24 | sed 's/^/\/\/ filler line /' > file.txt
printf 'other\n' > other.txt
jj describe -m "base" >/dev/null
base="$(jj log --no-graph -r @ -T 'commit_id')"
jj bookmark create litterbox/probe -r "$base" >/dev/null

# Two independent regions, each side adding its own line, so keeping both is the
# obvious resolution and the agent does not have to guess at intent.
rewrite() {
  sed -i "1s|.*|// $2|;20s|.*|fn $2() {}|" "$1"
}

# The user's side.
jj new "$base" -m "user: fold the retry helper into the client" >/dev/null
rewrite file.txt "user: fold the retry helper into the client"
theirs="$(jj log --no-graph -r @ -T 'commit_id')"

# The sandbox's side.
jj new "$base" -m "sandbox: make the client retry with backoff" >/dev/null
rewrite file.txt "sandbox: make the client retry with backoff"
ours="$(jj log --no-graph -r @ -T 'commit_id')"

# The user merged and committed without resolving, then pushed.
jj new "$theirs" "$ours" -m "merge the retry work" >/dev/null
tip="$(jj log --no-graph -r @ -T 'commit_id')"
jj bookmark set --allow-backwards litterbox/probe -r "$tip" >/dev/null

# Leave the working copy on the conflicted commit, as a user's checkout would be.
# Bookkeeping goes outside the repository: jj snapshots anything written inside it.
printf '%s\n' "$ours" > "$dest.meta.sandbox"
printf '%s\n' "$tip" > "$dest.meta.conflicted"

echo "fixture:      $dest"
echo "bookmark tip: ${tip:0:12} (conflicted)"
echo "sandbox tip:  ${ours:0:12}"
jj resolve --list -r 'litterbox/probe' || true
