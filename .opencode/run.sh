#!/usr/bin/env bash
set -eu -o pipefail

export OPENCODE_CONFIG_DIR=$(pwd)/.opencode

cargo build --bin agentkit-switchboard >/dev/null 2>&1

if command -v fuser &>/dev/null; then
  fuser -k 3812/tcp 2>/dev/null || true
elif command -v lsof &>/dev/null; then
  existing_pid=$(lsof -t -i:3812 2>/dev/null || true)
  if [ -n "$existing_pid" ]; then
    kill -9 "$existing_pid" 2>/dev/null || true
  fi
fi

./target/debug/agentkit-switchboard --config .opencode/switchboard.toml &
SWITCHBOARD_PID=$!

cleanup() {
  kill -TERM "$SWITCHBOARD_PID" 2>/dev/null || true
  wait "$SWITCHBOARD_PID" 2>/dev/null || true
  if command -v fuser &>/dev/null; then
    fuser -k 3812/tcp 2>/dev/null || true
  elif command -v lsof &>/dev/null; then
    existing_pid=$(lsof -t -i:3812 2>/dev/null || true)
    if [ -n "$existing_pid" ]; then
      kill -9 "$existing_pid" 2>/dev/null || true
    fi
  fi
}

trap cleanup EXIT INT TERM

sleep 1

exec opencode "$@"
