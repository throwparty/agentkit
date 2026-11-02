#!/usr/bin/env bash
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

cargo build --release --manifest-path "$here/bwrap/Cargo.toml"
cargo test --release --manifest-path "$here/bwrap/Cargo.toml"

for script in run failclosed attribution golden lifecycle lifecycle-cgroup limits edge egress portability; do
    echo "== $script"
    set +e
    bash "$here/$script.sh"
    status=$?
    set -e
    if [[ $status -eq 69 ]]; then
        echo "$script: skipped (no usable user manager)"
    elif [[ $status -ne 0 ]]; then
        exit "$status"
    fi
done

echo "== timing"
python3 "$here/timing.py"
