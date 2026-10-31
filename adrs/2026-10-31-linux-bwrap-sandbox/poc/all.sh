#!/usr/bin/env bash
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

cargo build --release --manifest-path "$here/bwrap/Cargo.toml"
cargo test --release --manifest-path "$here/bwrap/Cargo.toml"

for script in run failclosed attribution golden lifecycle edge egress portability; do
    echo "== $script"
    bash "$here/$script.sh"
done

echo "== timing"
python3 "$here/timing.py"
