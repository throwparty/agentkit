set shell := ["bash", "-eux", "-o", "pipefail", "-c"]

default: list

list:
    just --list --justfile {{source_directory()}}/mod.just

lint:
    nix --extra-experimental-features 'nix-command flakes' flake check --show-trace {{source_directory()}}
