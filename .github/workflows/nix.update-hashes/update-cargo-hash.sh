#!/usr/bin/env bash
set -euo pipefail

extract_hash() {
  local nix_output="$1"
  grep -oP 'got:\s*\Ksha256-[A-Za-z0-9+/=]+' <<< "$nix_output" | head -1 || true
}

update_flake_nix() {
  local new_hash="$1"
  local flake_file="${2:-nix/flake.nix}"
  local before
  before=$(cat "$flake_file")
  echo "Updating cargoHash to $new_hash" >&2
  sed -i 's|^\( *\)cargoHash = "sha256-[A-Za-z0-9+/=]*";|\1cargoHash = "'"$new_hash"'";|' "$flake_file"
  if [[ "$before" == "$(cat "$flake_file")" ]]; then
    echo "No cargoHash line found to update." >&2
    return 0
  fi
}

main() {
  local nix_output
  nix_output=$(nix --extra-experimental-features 'nix-command flakes' build --no-link \
    ./nix#agentkit-lens.cargoDeps \
    ./nix#agentkit-litterbox.cargoDeps 2>&1) || true

  if [[ -z "$nix_output" ]]; then
    echo "All checks passed, no update needed."
    return 0
  fi

  echo "$nix_output" >&2

  local new_hash
  new_hash=$(extract_hash "$nix_output")

  if [[ -z "$new_hash" ]]; then
    echo "No hash mismatch detected." >&2
    return 0
  fi

  update_flake_nix "$new_hash"
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
