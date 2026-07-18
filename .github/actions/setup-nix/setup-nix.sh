#!/usr/bin/env bash
set -euo pipefail

main() {
  sudo mkdir -p -m 0755 /nix
  sudo chown "$USER" /nix
  curl -L https://nixos.org/nix/install | sh -s -- --no-daemon
  echo "${HOME}/.nix-profile/bin" >> "${GITHUB_PATH}"
  nix --version
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
