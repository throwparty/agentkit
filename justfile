mod workflows ".github/workflows"

set shell := ["bash", "-eux", "-o", "pipefail", "-c"]

default: list

list:
    just --list

fmt:
    treefmt

lint:
    just workflows:lint
    cargo clippy --all-targets --all-features -- -D warnings

build:
