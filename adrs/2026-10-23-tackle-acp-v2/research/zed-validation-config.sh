#!/usr/bin/env bash
# Build an isolated Tackle config for the manual Zed validation run.
#
# Copies the system user config and appends the test-only filesystem fixture
# stanza, leaving the system config untouched. Idempotent: re-running keeps an
# existing [mcp_servers.testfs] stanza.
#
# Usage:
#   zed-validation-config.sh [DEST_DIR] [PROJECT_ROOT] [FIXTURE_BIN]
#
# Defaults:
#   DEST_DIR      <repo>/target/zed-validation-config
#   PROJECT_ROOT  <repo>
#   FIXTURE_BIN   <repo>/target/debug/tackle-mcp-fs
#   SOURCE        $TACKLE_SOURCE_CONFIG, else the platform user config
#
# Set TACKLE_SOURCE_CONFIG to override the system config path.
set -euo pipefail

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
    sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
dest="${1:-$repo_root/target/zed-validation-config}"
project_root="${2:-$repo_root}"
fixture="${3:-$repo_root/target/debug/tackle-mcp-fs}"

if [[ -z "${TACKLE_SOURCE_CONFIG:-}" ]]; then
    case "$(uname -s)" in
        Darwin)
            TACKLE_SOURCE_CONFIG="$HOME/Library/Application Support/AgentKit/tackle/config.toml"
            ;;
        *)
            TACKLE_SOURCE_CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/agentkit/tackle/config.toml"
            ;;
    esac
fi

mkdir -p "$dest"
dest="$(cd "$dest" && pwd)"

if [[ -f "$TACKLE_SOURCE_CONFIG" ]]; then
    cp "$TACKLE_SOURCE_CONFIG" "$dest/config.toml"
    echo "copied  $TACKLE_SOURCE_CONFIG"
    echo "     -> $dest/config.toml"
else
    : > "$dest/config.toml"
    echo "no system config at $TACKLE_SOURCE_CONFIG; starting empty"
fi

if [[ ! -x "$fixture" ]]; then
    echo "warning: fixture not found or not executable: $fixture" >&2
    echo "         build it with: nix develop ./nix --command cargo build -p agentkit-tackle --bin tackle-mcp-fs" >&2
fi

if grep -qE '^\[mcp_servers\.testfs\]' "$dest/config.toml"; then
    echo "kept    existing [mcp_servers.testfs] stanza"
else
    cat >> "$dest/config.toml" <<STANZA

[mcp_servers.testfs]
transport = "stdio"
command = "$fixture"
args = ["--root", "$project_root"]
STANZA
    echo "appended [mcp_servers.testfs]"
fi

cat <<SUMMARY

Run's config: $dest/config.toml
Run's sessions: $repo_root/target/zed-validation.db

Zed agent_servers entry:
  "args": ["stdio", "--config-dir", "$dest", "--db-path", "$repo_root/target/zed-validation.db"]

Remove the stanza (or the directory) when the run is finished.
SUMMARY
