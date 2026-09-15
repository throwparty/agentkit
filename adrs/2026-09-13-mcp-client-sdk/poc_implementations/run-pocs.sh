#!/usr/bin/env bash
# Run both MCP client PoCs against the same shared mcp-server and show their
# output side by side. Fails loudly if either PoC does not complete the full
# flow (handshake, tool list, tool call).
set -euo pipefail

cd "$(dirname "$0")"

echo "==> building shared mcp-server + both clients"
cargo build --workspace

echo
echo "==> running poc-rmcp"
rmcp_out="$(mktemp)"
rmcp_err="$(mktemp)"
if timeout 60 cargo run --quiet -p poc-rmcp >"$rmcp_out" 2>"$rmcp_err"; then
    rmcp_ok="ok"
else
    rmcp_ok="FAILED (see $rmcp_err)"
fi

echo
echo "==> running poc-rust-mcp-sdk"
sdk_out="$(mktemp)"
sdk_err="$(mktemp)"
if timeout 60 cargo run --quiet -p poc-rust-mcp-sdk >"$sdk_out" 2>"$sdk_err"; then
    sdk_ok="ok"
else
    sdk_ok="FAILED (see $sdk_err)"
fi

echo
echo "==> poc-rmcp ($rmcp_ok)"
cat "$rmcp_out"
echo
echo "==> poc-rust-mcp-sdk ($sdk_ok)"
cat "$sdk_out"

if [[ "$rmcp_ok" != "ok" || "$sdk_ok" != "ok" ]]; then
    echo
    echo "!! one or both PoCs failed"
    exit 1
fi

echo
echo "==> both PoCs completed against the same shared mcp-server"