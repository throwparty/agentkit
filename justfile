set shell := ["bash", "-eux", "-o", "pipefail", "-c"]

default: list

list:
    just --list

fmt:
    treefmt

lint:
    cargo clippy --all-targets --all-features -- -D warnings

build:
