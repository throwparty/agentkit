#!/usr/bin/env bash
set -euo pipefail

echo "kernel: $(uname -r)"
echo "command: unshare -U true"
set +e
unshare -U true
status=$?
set -e
echo "exit: $status"
if [[ $status -ne 0 ]]; then
    exit 1
fi

command -v bwrap
bwrap --version
