set shell := ["bash", "-eux", "-o", "pipefail", "-c"]

default: list

list:
    just --list --justfile {{source_directory()}}/mod.just

serve-e2e:
    cargo run -p agentkit-switchboard -- --config crates/agentkit-switchboard/e2e.toml

serve-local:
    cargo run -p agentkit-switchboard
