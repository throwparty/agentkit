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
#   fixture — committed tackle config, conformance inputs, and the baseline.
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
    cargo build -p agentkit-tackle --bin agentkit-tackle --bin tackle-test-model --bin tackle-conformance-check

start-test-model:
    #!/usr/bin/env bash
    set -euxo pipefail
    # The orchestration passes a per-invocation pid file; manual runs use the shared default.
    pid_file=${TEST_MODEL_PID_FILE:-{{ artifacts }}/test-model.pid}
    mkdir -p {{ artifacts }}
    if timeout 1 bash -c "exec 3<>/dev/tcp/{{ test_model_bind }}/{{ test_model_port }}" 2>/dev/null; then
        echo "error: port {{ test_model_port }} on {{ test_model_bind }} is already in use" >&2
        exit 1
    fi
    rm -f "$pid_file"
    target/debug/tackle-test-model --bind {{ test_model_bind }} --port {{ test_model_port }} --cancel-sentinel '{{ cancel_sentinel }}' > {{ artifacts }}/test-model.log 2>&1 &
    pid=$!
    echo "$pid" > "$pid_file"
    cleanup() {
        rc=$?
        if [ "$rc" -ne 0 ] && kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
            rm -f "$pid_file"
        fi
        exit "$rc"
    }
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    trap 'exit 129' HUP
    for _ in {1..300}; do
        if ! kill -0 "$pid" 2>/dev/null; then
            echo "error: test model exited before binding {{ test_model_bind }}:{{ test_model_port }}" >&2
            cat {{ artifacts }}/test-model.log >&2 || true
            exit 1
        fi
        if timeout 1 bash -c "exec 3<>/dev/tcp/{{ test_model_bind }}/{{ test_model_port }}" 2>/dev/null; then
            trap - EXIT INT TERM HUP
            exit 0
        fi
        sleep 0.1
    done
    echo "error: test model did not bind {{ test_model_bind }}:{{ test_model_port }} within 30s" >&2
    cat {{ artifacts }}/test-model.log >&2 || true
    exit 1

stop-test-model:
    #!/usr/bin/env bash
    set -euxo pipefail
    # The orchestration passes a per-invocation pid file; manual runs use the shared default.
    pid_file=${TEST_MODEL_PID_FILE:-{{ artifacts }}/test-model.pid}
    if [ -f "$pid_file" ]; then
        pid=$(cat "$pid_file")
        rm -f "$pid_file"
        if kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
            for _ in {1..50}; do
                kill -0 "$pid" 2>/dev/null || break
                sleep 0.1
            done
            kill -KILL "$pid" 2>/dev/null || true
        fi
    fi

run-tck:
    #!/usr/bin/env bash
    set -euxo pipefail
    mkdir -p {{ artifacts }} {{ artifacts }}/workdir
    rm -f {{ artifacts }}/report.json
    rm -f {{ artifacts }}/sessions.db {{ artifacts }}/sessions.db-wal {{ artifacts }}/sessions.db-shm
    rc=0
    acp-tck \
        --protocol-version 1 \
        --timeout 30 \
        --startup-timeout 30 \
        --test-timeout 120 \
        --agent-cwd "$PWD/{{ artifacts }}/workdir" \
        --cancel-prompt '{{ cancel_sentinel }}' \
        --report-json {{ artifacts }}/report.json \
        -- "$PWD/target/debug/agentkit-tackle" stdio --config-dir '{{ fixture }}' --db-path "$PWD/{{ artifacts }}/sessions.db" \
        || rc=$?
    case "$rc" in
        0) ;;
        1)
            if [ ! -s {{ artifacts }}/report.json ]; then
                echo "error: acp-tck exited 1 without writing a report; harness failure, not a conformance verdict" >&2
                exit 1
            fi
            echo "acp-tck exited 1; report written, deferring to the baseline diff" >&2
            ;;
        *)
            echo "error: acp-tck exited $rc; harness failure, not a conformance verdict" >&2
            exit "$rc"
            ;;
    esac

# Compare the latest report against the committed baseline.
check-baseline:
    #!/usr/bin/env bash
    set -euxo pipefail
    target/debug/tackle-conformance-check \
        --report {{ artifacts }}/report.json \
        --baseline '{{ fixture }}/baseline.txt'

# Regenerate the baseline from the latest report after reviewing its failures.
seed-baseline:
    #!/usr/bin/env bash
    set -euxo pipefail
    target/debug/tackle-conformance-check \
        --report {{ artifacts }}/report.json \
        --baseline '{{ fixture }}/baseline.txt' \
        --seed

# Single entry point for developers and CI: build, run the suite, stop the fixture.
conformance:
    #!/usr/bin/env bash
    set -euxo pipefail
    export TEST_MODEL_PID_FILE="{{ artifacts }}/test-model.$$.pid"
    mkdir -p {{ artifacts }}
    rm -f {{ artifacts }}/report.json
    trap 'just --justfile {{ source_directory() }}/mod.just stop-test-model' EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    trap 'exit 129' HUP
    just --justfile {{ source_directory() }}/mod.just build
    just --justfile {{ source_directory() }}/mod.just start-test-model
    just --justfile {{ source_directory() }}/mod.just run-tck
    just --justfile {{ source_directory() }}/mod.just check-baseline
