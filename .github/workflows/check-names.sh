#!/usr/bin/env bash
set -euo pipefail

main() {
  local files=("$@")
  if [ ${#files[@]} -eq 0 ]; then
    local dir
    dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    files=("$dir"/*.yaml)
  fi
  local errors=0
  for f in "${files[@]}"; do
    if yq eval '.jobs.*.steps[] | select(.name == null and (.uses != null or .run != null))' "$f" 2>/dev/null | grep -q .; then
      echo "$f: step missing name"
      errors=1
    fi
  done
  exit "$errors"
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
