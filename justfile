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
    bash hack/arch-lint-negative.sh

shellcheck:
    shellcheck $(git ls-files '*.sh' '*.bash' '*.bats')
    shellcheck --exclude=SC2148 $(git ls-files '*.envrc')

test:
    just workflows test

build:
