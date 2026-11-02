#!/usr/bin/env bash
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
harness="$here/bwrap/target/release/lbx-bwrap"

cargo build --release --manifest-path "$here/bwrap/Cargo.toml" >/dev/null

workspace=$(mktemp -d /tmp/lbx-egress.XXXXXX)
port_file=$(mktemp /tmp/lbx-port.XXXXXX)
trap 'kill "$server" 2>/dev/null || true; rm -rf "$workspace" "$port_file"' EXIT

python3 "$here/echo_server.py" >"$port_file" &
server=$!

for _ in $(seq 50); do
    if [[ -s "$port_file" ]]; then
        break
    fi
    sleep 0.1
done
port=$(cat "$port_file")
if [[ -z "$port" ]]; then
    echo "egress: echo server did not start"
    exit 1
fi

host_ip=$(hostname -I 2>/dev/null | awk '{print $1}')
if [[ -z "$host_ip" ]]; then
    host_ip=127.0.0.1
fi

probe="import socket,sys; s=socket.create_connection((sys.argv[1], int(sys.argv[2])), 2); s.sendall(b'ping'); sys.exit(0 if s.recv(4) == b'ping' else 1)"

if ! python3 -c "$probe" "$host_ip" "$port"; then
    echo "egress: the control could not reach $host_ip:$port"
    exit 1
fi

if ! "$harness" --workspace "$workspace" -- python3 -c "$probe" "$host_ip" "$port" 2>/dev/null; then
    echo "egress: host echo server reached by the control, unreachable from the sandbox"
else
    echo "egress: the sandbox reached $host_ip:$port"
    exit 1
fi
