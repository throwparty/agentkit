#!/usr/bin/env bash
set -euo pipefail

main() {
  local github_token="${GITHUB_TOKEN:-}"
  local repo="${GITHUB_REPOSITORY:-}"

  if ! git diff --exit-code --quiet nix/flake.nix; then
    git config user.name github-actions[bot]
    git config user.email github-actions[bot]@users.noreply.github.com
    git add nix/flake.nix
    git commit -m "Update Nix cargo hash after dependency change"
    git remote set-url origin "https://x-access-token:${github_token}@github.com/${repo}"
    git push
  fi
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
