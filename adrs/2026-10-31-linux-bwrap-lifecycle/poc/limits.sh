#!/usr/bin/env bash
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
harness="$here/bwrap/target/release/lbx-bwrap"

cargo build --release --manifest-path "$here/bwrap/Cargo.toml" >/dev/null

if ! systemctl --user show-environment >/dev/null 2>&1; then
    echo "limits: no user manager"
    exit 69
fi

workspace=$(mktemp -d /tmp/lbx-limits.XXXXXX)
log_dir=$(mktemp -d /tmp/lbx-limits-log.XXXXXX)
unit=""
cleanup() {
    if [[ -n "$unit" ]]; then
        systemctl --user stop "$unit.scope" 2>/dev/null || true
    fi
    rm -rf "$workspace" "$log_dir"
}
trap cleanup EXIT

run_scope() {
    local name=$1
    shift
    unit=$name
    systemd-run --user --scope --quiet --unit="$unit" "$@" >"$log_dir/$unit.out" 2>"$log_dir/$unit.err"
}

memory_props=(-p MemoryMax=64M -p MemorySwapMax=0)
task_props=(-p TasksMax=16)

unit="lbx-mem-control-$$"
if ! run_scope "$unit" "${memory_props[@]}" -- "$harness" --workspace "$workspace" -- \
    python3 -c 'x = bytearray(8 * 1024 * 1024); print(len(x))'; then
    echo "limits: the memory control failed"
    cat "$log_dir/$unit.err" >&2
    exit 1
fi
systemctl --user stop "$unit.scope" 2>/dev/null || true

unit="lbx-mem-hog-$$"
if run_scope "$unit" "${memory_props[@]}" -- "$harness" --workspace "$workspace" -- \
    python3 -c 'x = bytearray(256 * 1024 * 1024); print(len(x))'; then
    echo "limits: the memory hog succeeded"
    exit 1
fi
result=$(systemctl --user show "$unit.scope" -p Result --value 2>/dev/null || true)
systemctl --user stop "$unit.scope" 2>/dev/null || true
if [[ "$result" != "oom-kill" ]]; then
    echo "limits: the memory hog failed without an OOM kill (scope result ${result:-unavailable})"
    cat "$log_dir/$unit.out" "$log_dir/$unit.err" >&2
    exit 1
fi
echo "limits: memory hog bounded (scope result $result)"

unit="lbx-tasks-control-$$"
if ! run_scope "$unit" "${task_props[@]}" -- "$harness" --workspace "$workspace" -- \
    python3 -c 'import threading,time; ts=[threading.Thread(target=time.sleep,args=(1,)) for _ in range(4)]; [t.start() for t in ts]; [t.join() for t in ts]'; then
    echo "limits: the task control failed"
    cat "$log_dir/$unit.err" >&2
    exit 1
fi
systemctl --user stop "$unit.scope" 2>/dev/null || true

cat >"$workspace/threads.py" <<'PY'
import threading, time
try:
    threads = [threading.Thread(target=time.sleep, args=(1,)) for _ in range(64)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
except RuntimeError as error:
    print(f"thread-limit: {error}")
    raise SystemExit(42)
PY

unit="lbx-tasks-hog-$$"
set +e
run_scope "$unit" "${task_props[@]}" -- "$harness" --workspace "$workspace" -- \
    python3 /src/threads.py
status=$?
set -e
if [[ $status -eq 0 ]]; then
    echo "limits: the thread hog succeeded"
    exit 1
fi
if ! grep -q 'thread-limit:' "$log_dir/$unit.out" "$log_dir/$unit.err"; then
    echo "limits: the thread hog did not report the task limit"
    cat "$log_dir/$unit.out" "$log_dir/$unit.err" >&2
    exit 1
fi
systemctl --user stop "$unit.scope" 2>/dev/null || true
echo "limits: task hog bounded (thread creation refused)"
