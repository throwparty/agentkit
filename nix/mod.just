set shell := ["bash", "-eux", "-o", "pipefail", "-c"]

default: list

list:
    just --list --justfile {{source_directory()}}/mod.just

lint:
    nix --extra-experimental-features 'nix-command flakes' flake show {{source_directory()}}
    nix --extra-experimental-features 'nix-command flakes' build {{source_directory()}}#agentkit-lens.cargoDeps --no-link
    nix --extra-experimental-features 'nix-command flakes' build {{source_directory()}}#agentkit-litterbox.cargoDeps --no-link

update-hash:
    .github/workflows/nix.update-hashes/update-cargo-hash.sh
