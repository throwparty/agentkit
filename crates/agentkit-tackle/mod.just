# Conformance runner for tackle.
#
# The pinned acp-tck suite drives the tackle binary over stdio. Tackle needs an
# OpenAI-compatible model server to answer prompts, so the suite runs against
# tackle-test-model, a deterministic fixture process started here and stopped
# afterwards.
#
# Variables:
#   test_model_bind/test_model_port — the address the fixture listens on; the
#     config fixture and the readiness probe both point here.
#   cancel_sentinel — prompt text that makes the fixture hold the stream open,
#     so acp-tck's cancellation requirements are exercised.
#   fixture — committed tackle config and conformance inputs.
#   artifacts — run output (pid, log, report) under the ignored target tree.

set shell := ["bash", "-eux", "-o", "pipefail", "-c"]
set working-directory := "../.."

test_model_bind := "127.0.0.1"
test_model_port := "45123"
cancel_sentinel := "TACKLE-CONFORMANCE-CANCEL"
fixture := source_directory() + "/tests/conformance"
artifacts := "target/conformance"

default: list

list:
    just --list --justfile {{ source_directory() }}/mod.just

build:
    cargo build -p agentkit-tackle --bin agentkit-tackle --bin tackle-test-model

start-test-model:
    #!/usr/bin/env bash
    set -euxo pipefail
    mkdir -p {{ artifacts }}
    target/debug/tackle-test-model --bind {{ test_model_bind }} --port {{ test_model_port }} --cancel-sentinel '{{ cancel_sentinel }}' > {{ artifacts }}/test-model.log 2>&1 &
    echo $! > {{ artifacts }}/test-model.pid
    timeout 30 bash -c 'until exec 3<>/dev/tcp/{{ test_model_bind }}/{{ test_model_port }}; do sleep 0.1; done'
    kill -0 "$(cat {{ artifacts }}/test-model.pid)"

stop-test-model:
    #!/usr/bin/env bash
    set -euxo pipefail
    if [ -f {{ artifacts }}/test-model.pid ]; then
        kill "$(cat {{ artifacts }}/test-model.pid)" 2>/dev/null || true
        rm -f {{ artifacts }}/test-model.pid
    fi

run-tck:
    #!/usr/bin/env bash
    set -euxo pipefail
    mkdir -p {{ artifacts }} {{ artifacts }}/workdir
    rm -f {{ artifacts }}/sessions.db {{ artifacts }}/sessions.db-wal {{ artifacts }}/sessions.db-shm
    acp-tck \
        --protocol-version 1 \
        --timeout 30 \
        --startup-timeout 30 \
        --test-timeout 120 \
        --agent-cwd "$PWD/{{ artifacts }}/workdir" \
        --cancel-prompt '{{ cancel_sentinel }}' \
        --report-json {{ artifacts }}/report.json \
        -- "$PWD/target/debug/agentkit-tackle" stdio --config-dir '{{ fixture }}' --db-path "$PWD/{{ artifacts }}/sessions.db"

# Single entry point for developers and CI: build, run the suite, stop the fixture.
conformance:
    #!/usr/bin/env bash
    set -euxo pipefail
    just --justfile {{ source_directory() }}/mod.just build
    just --justfile {{ source_directory() }}/mod.just start-test-model
    trap 'just --justfile {{ source_directory() }}/mod.just stop-test-model' EXIT
    just --justfile {{ source_directory() }}/mod.just run-tck
