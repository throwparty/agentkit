#!/usr/bin/env bash
set -euo pipefail

main() {
  pip install zizmor
  zizmor "$@"
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
