#!/usr/bin/env bash
set -euo pipefail

main() {
  git config --global --add url."https://github.com/".insteadOf "git+ssh://git@github.com/"
  git config --global --add url."https://github.com/".insteadOf "ssh://git@github.com/"
  git config --global --add url."https://github.com/".insteadOf "git@github.com:"
  git config --global --get-all url."https://github.com/".insteadOf
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
