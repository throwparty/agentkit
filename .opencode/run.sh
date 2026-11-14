#!/usr/bin/env bash
set -eu -o pipefail

# Run the launcher under the repository's Nix development shell. It carries the
# Rust toolchain, dbus and OpenSSL the built binaries link against, and the
# credential helper resolves from the build directory below.
if [ "${IN_NIX_SHELL:-}" != "impure" ]; then
  exec nix develop ./nix --command bash "$0" "$@"
fi

export OPENCODE_CONFIG_DIR
OPENCODE_CONFIG_DIR=$(pwd)/.opencode

cargo build \
  --bin agentkit-switchboard \
  --bin agentkit-credential-keychain \
  --bin agentkit-lens \
  --bin agentkit-litterbox
export PATH="$PWD/target/debug:$PATH"

# Kill whatever is listening on a port. lsof is preferred because it is present
# on both macOS and Linux and takes the same arguments on each. fuser only
# exists on macOS as the BSD tool, which has different flags and no idea what
# "3812/tcp" means, so it is only used when it is the psmisc implementation.
free_port() {
  local port=$1
  if command -v lsof &>/dev/null; then
    pids=$(lsof -t -i "tcp:$port" 2>/dev/null || true)
    if [ -n "$pids" ]; then
      # Word splitting is intended: lsof may report several PIDs.
      # shellcheck disable=SC2086
      kill -9 $pids 2>/dev/null || true
    fi
  elif command -v fuser &>/dev/null && fuser --version 2>&1 | grep -qi psmisc; then
    fuser -k "$port/tcp" >/dev/null 2>&1 || true
  fi
}

free_port 3812

# The kill above is fire-and-forget: do not start our instance until 3812
# is actually free, otherwise the readiness probe below can get a 200 from
# a stale server and report ready for an instance that already exited.
for _ in $(seq 1 50); do
  if ! (echo >/dev/tcp/127.0.0.1/3812) >/dev/null 2>&1; then
    break
  fi
  sleep 0.2
done
if (echo >/dev/tcp/127.0.0.1/3812) >/dev/null 2>&1; then
  echo "error: port 3812 is still occupied by another process." >&2
  echo "error: free it (e.g. lsof -t -i tcp:3812 | xargs kill) and retry; refusing to launch opencode against a stale backend." >&2
  exit 1
fi

SWITCHBOARD_LOG_FILE=$(mktemp /tmp/agentkit-switchboard.XXXXXX.log)
echo "agentkit-switchboard will log to $SWITCHBOARD_LOG_FILE" >&2
./target/debug/agentkit-switchboard --config .opencode/switchboard.toml \
  >"$SWITCHBOARD_LOG_FILE" 2>&1 &
SWITCHBOARD_PID=$!

cleanup() {
  if [ -n "${SWITCHBOARD_PID:-}" ]; then
    kill -TERM "$SWITCHBOARD_PID" 2>/dev/null || true
    wait "$SWITCHBOARD_PID" 2>/dev/null || true
  fi
  free_port 3812
}

trap cleanup EXIT INT TERM

echo -n "waiting for agentkit-switchboard to be ready" >&2
ready=0
for _ in $(seq 1 100); do
  if ! kill -0 "$SWITCHBOARD_PID" 2>/dev/null; then
    break
  fi
  sleep 0.1
  echo -n .
  health=$(curl -fsS --max-time 2 http://127.0.0.1:3812/health 2>/dev/null || true)
  case "${health//[[:space:]]/}" in
    *'"status":"ok"'*)
      ready=1
      break
      ;;
  esac
done
echo ""

if [ "$ready" -ne 1 ]; then
  echo "error: agentkit-switchboard did not become ready" >&2
  if ! kill -0 "$SWITCHBOARD_PID" 2>/dev/null; then
    echo "error: switchboard process exited prematurely" >&2
  fi
  exit 1
fi

# The shared background service may have been started outside the devShell, and
# it is the server, not the TUI, that spawns MCP servers. A private server
# inherits this shell, so cargo and the built helpers are on its PATH.
opencode --standalone "$@"
