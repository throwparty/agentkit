#!/usr/bin/env bash
set -euo pipefail

main() {
  rustup toolchain install stable
  rustup default stable
  for component in "$@"; do
    rustup component add "$component"
  done
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
