#!/usr/bin/env bash
# Verify Seatbelt deny-rule masking of VCS metadata on a real macOS host.
#
# A Bound sandbox is created through the Seatbelt backend against a workspace
# carrying `.jj`. From inside the sandbox the probe reads and writes the visible
# workspace, confirms the masked entry is present in a listing, confirms read,
# list, stat and write under it return EPERM, and confirms a descendant shell
# inherits the denial. It then runs the renderer test that pins deny-after-allow
# and the Copied regression tests.
#
# Run from anywhere:
#   bash adrs/2026-11-21-hide-vcs-metadata-from-sandboxes/evidence/T-017/probe.sh

set -euo pipefail

repo_root="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
cd "$repo_root"
export CARGO_TERM_COLOR=never

printf 'host: macOS %s (%s)\n' "$(sw_vers -productVersion)" "$(uname -m)"
printf 'repo: %s\n' "$repo_root"

printf '\n== build the Seatbelt helper ==\n'
cargo build -p agentkit-litterbox --bin litterbox-seatbelt-helper 2>&1

run_test() {
  local label="$1"
  local filter="$2"
  local output
  local status

  printf '\n== %s ==\n' "$label"
  set +e
  output="$(cargo test -p agentkit-litterbox --lib "$filter" -- --nocapture 2>&1)"
  status=$?
  set -e
  printf '%s\n' "$output"
  if [ "$status" -ne 0 ]; then
    printf 'FAIL: %s failed\n' "$filter"
    exit "$status"
  fi
  if ! printf '%s\n' "$output" | grep -q '1 passed'; then
    printf 'FAIL: %s did not run exactly one test\n' "$filter"
    exit 1
  fi
}

run_test "Bound workspace: visible entry, EPERM on read/write/stat, descendant inheritance" \
  a_bound_workspace_masks_the_metadata_path
run_test "Renderer: the deny rules follow the allow rules" \
  a_masked_path_is_denied_after_the_allows
run_test "Copied: the renderer emits no deny layer" \
  a_copied_workspace_renders_no_deny_layer
run_test "Copied: create, exec and cleanup are unchanged" \
  create_exec_and_cleanup

printf '\nRESULT: every check passed\n'
