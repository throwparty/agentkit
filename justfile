mod docs "docs"
mod nix "nix"
mod switchboard "crates/agentkit-switchboard"
mod tackle "crates/agentkit-tackle"
mod workflows ".github/workflows"

set shell := ["bash", "-eux", "-o", "pipefail", "-c"]

default: list

list:
    just --list

fmt:
    treefmt

lint:
    just nix lint
    just workflows lint
    cargo clippy --all-targets --all-features -- -D warnings
    arch-lint check

test:
    just workflows test

build:
