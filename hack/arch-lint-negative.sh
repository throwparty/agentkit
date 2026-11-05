#!/usr/bin/env bash
# Proves the Litterbox module boundary is load-bearing.
#
# Injects imports the contract, the CLI, the MCP server and the registry must
# never make, expects `arch-lint check` to fail for each, and restores the tree
# on every exit path.
set -euo pipefail

CURRENT_FILE=""
CURRENT_BACKUP=""

restore() {
  if [[ -n "$CURRENT_BACKUP" && -n "$CURRENT_FILE" ]]; then
    cp "$CURRENT_BACKUP" "$CURRENT_FILE"
    rm -f "$CURRENT_BACKUP"
  fi
}

trap restore EXIT INT TERM

check_clean() {
  if ! arch-lint check >/dev/null 2>&1; then
    echo "arch-lint fails on a clean tree; fix that first" >&2
    exit 1
  fi
}

plant() {
  local file="$1" line="$2" label="$3" rule="$4"
  local backup output
  backup="$(mktemp)"
  cp "$file" "$backup"
  CURRENT_FILE="$file"
  CURRENT_BACKUP="$backup"
  printf '\n%s\n' "$line" >>"$file"

  if output="$(arch-lint check 2>&1)"; then
    cp "$backup" "$file"
    rm -f "$backup"
    CURRENT_BACKUP=""
    echo "arch-lint accepted a $label import" >&2
    exit 1
  fi

  cp "$backup" "$file"
  rm -f "$backup"
  CURRENT_BACKUP=""

  if ! printf '%s' "$output" | grep -q "$rule"; then
    echo "arch-lint failed for an unexpected reason; wanted $rule:" >&2
    printf '%s\n' "$output" >&2
    exit 1
  fi

  echo "arch-lint rejected the planted $label import ($rule)"
}

check_clean

plant "crates/agentkit-litterbox/src/sandbox/mod.rs" \
  "use crate::backends::docker::BACKEND as _;" \
  "contract-to-backend" \
  "litterbox-contract"

plant "crates/agentkit-litterbox/src/cli.rs" \
  "use crate::backends::docker::BACKEND as _;" \
  "cli-to-backend" \
  "litterbox-cli"

plant "crates/agentkit-litterbox/src/cli.rs" \
  "use crate::scm::Scm as _;" \
  "cli-to-scm" \
  "litterbox-cli"

plant "crates/agentkit-litterbox/src/cli.rs" \
  "use crate::provision::resolve as _;" \
  "cli-to-provision" \
  "litterbox-cli"

plant "crates/agentkit-litterbox/src/mcp.rs" \
  "use crate::provision::resolve as _;" \
  "mcp-to-provision" \
  "litterbox-mcp"

plant "crates/agentkit-litterbox/src/config.rs" \
  "use crate::backends::docker::BACKEND as _;" \
  "config-to-backend" \
  "litterbox-config"

plant "crates/agentkit-litterbox/src/config.rs" \
  "use crate::provision::resolve as _;" \
  "config-to-provision" \
  "litterbox-config"

plant "crates/agentkit-litterbox/src/sandbox/mod.rs" \
  "use crate::provision::resolve as _;" \
  "contract-to-provision" \
  "litterbox-contract"

plant "crates/agentkit-litterbox/src/backends/registry.rs" \
  "use crate::orchestrator::Sandboxes as _;" \
  "registry-to-orchestrator" \
  "litterbox-registry"

plant "crates/agentkit-litterbox/src/backends/registry.rs" \
  "use crate::provision::resolve as _;" \
  "registry-to-provision" \
  "litterbox-registry"

plant "crates/agentkit-litterbox/src/orchestrator/service.rs" \
  "use crate::backends::docker::BACKEND as _;" \
  "orchestrator-to-backend" \
  "litterbox-orchestrator"

plant "crates/agentkit-litterbox/src/backends/docker/backend.rs" \
  "use crate::provision::resolve as _;" \
  "backend-to-provision" \
  "litterbox-backend-docker"

plant "crates/agentkit-litterbox/src/provision/resolve.rs" \
  "use crate::orchestrator::Sandboxes as _;" \
  "provision-to-orchestrator" \
  "litterbox-provision"
