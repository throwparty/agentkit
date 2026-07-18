#!/usr/bin/env bash
set -euo pipefail

main() {
  bash <(curl -sS https://raw.githubusercontent.com/rhysd/actionlint/main/scripts/download-actionlint.bash)
  ./actionlint -ignore 'unexpected key "queue" for "concurrency" section' "$@"
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
