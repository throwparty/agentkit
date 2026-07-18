#!/usr/bin/env bash
set -euo pipefail

main() {
  exec nix --extra-experimental-features 'nix-command flakes' flake check --show-trace "$@"
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
