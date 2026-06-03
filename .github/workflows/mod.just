set shell := ["bash", "-eux", "-o", "pipefail", "-c"]

default: list

list:
    just --list --justfile {{source_directory()}}/mod.just

actionlint:
    actionlint

zizmor:
    zizmor {{source_directory()}}

lint: zizmor actionlint

ratchet command:
     find . -type f -path "*.yaml" \
            -exec ratchet {{command}} {} \;
