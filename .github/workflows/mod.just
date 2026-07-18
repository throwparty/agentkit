set shell := ["bash", "-eux", "-o", "pipefail", "-c"]

default: list

list:
    just --list --justfile {{source_directory()}}/mod.just

actionlint:
    actionlint -ignore 'unexpected key "queue" for "concurrency" section'

zizmor *flags:
    zizmor --persona=auditor {{flags}} {{source_directory()}}

test-actions:
    bats ../actions/*/*.bats

test-workflows:
    bats */*.bats

test: test-actions test-workflows

lint: zizmor actionlint test

ratchet command:
     find . -type f -path "*.yaml" \
            -exec ratchet {{command}} {} \;
