#!/usr/bin/env bash
# T-013: the current Litterbox module layout, size and Git coupling.
set -euo pipefail

root="$(cd "$(dirname "$0")/../../../.." && pwd)"
cd "$root"

echo '=== modules ==='
find crates/agentkit-litterbox/src -maxdepth 2 -type f -name '*.rs' | sort

echo '=== line counts ==='
find crates/agentkit-litterbox/src -type f -name '*.rs' -print0 \
  | xargs -0 wc -l | sort -rn | head -30

echo '=== crate modules ==='
sed -n '1,30p' crates/agentkit-litterbox/src/lib.rs

echo '=== Git-specific references outside scm/vcs_store ==='
grep -rn 'git2\|ScmMode\|VcsStore\|ThreadSafeScm\|commit_snapshot\|make_archive' \
  crates/agentkit-litterbox/src/orchestrator \
  crates/agentkit-litterbox/src/cli.rs \
  crates/agentkit-litterbox/src/mcp.rs \
  crates/agentkit-litterbox/src/metadata_store.rs \
  | head -80

echo '=== orchestrator service functions ==='
grep -n '^\s*\(pub \)\?\(async \)\?fn ' crates/agentkit-litterbox/src/orchestrator/service.rs | head -60

echo '=== mcp tool handlers ==='
grep -n '#\[tool(' crates/agentkit-litterbox/src/mcp.rs
