#!/usr/bin/env bash
set -euo pipefail

# Generic MCP Test Harness for write_file PoC implementations
# This script tests the MCP server by sending JSON-RPC messages over stdio
#
# Usage: ./test_mcp_server.sh <poc-directory>
# Example: ./test_mcp_server.sh poc-rmcp

if [ $# -lt 1 ]; then
    echo "Usage: $0 <poc-directory>"
    echo "Example: $0 poc-rmcp"
    exit 1
fi

POC_DIR="$1"
POC_NAME=$(basename "$POC_DIR")
TEST_DIR="/tmp/mcp_test_${POC_NAME}_$$"
TEST_FILE="$TEST_DIR/nested/test.txt"

# Colors for output
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${GREEN}[INFO]${NC} $*"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $*"
}

log_test() {
    echo -e "${YELLOW}[TEST]${NC} $*"
}

log_header() {
    echo -e "${BLUE}[====]${NC} $*"
}

cleanup() {
    log_info "Cleaning up test directory: $TEST_DIR"
    rm -rf "$TEST_DIR"
}

trap cleanup EXIT

log_header "Testing MCP PoC: $POC_NAME"
log_info "PoC directory: $POC_DIR"

# Build the server
log_info "Building MCP server..."
(cd "$POC_DIR" && cargo build --quiet)

# Create test directory
log_info "Creating test directory: $TEST_DIR"
mkdir -p "$TEST_DIR"

# Test 1: Initialize the MCP connection
log_test "Test 1: Initialize MCP connection"
INIT_REQUEST=$(cat <<EOF
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{"roots":{"listChanged":true}},"clientInfo":{"name":"test-harness","version":"1.0.0"}}}
EOF
)

INIT_RESPONSE=$( (echo "$INIT_REQUEST"; sleep 0.5) | "$SERVER_BIN" 2>/dev/null | head -1)
log_info "Initialize response: $INIT_RESPONSE"

if echo "$INIT_RESPONSE" | grep -q '"result"'; then
    log_info "✓ Initialize successful"
else
    log_error "✗ Initialize failed"
    exit 1
fi

# Test 2: List available tools
log_test "Test 2: List available tools"

# We need to send initialize + initialized notification + list tools
SEQUENCE=$(cat <<EOF
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{"roots":{"listChanged":true}},"clientInfo":{"name":"test-harness","version":"1.0.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
EOF
)

TOOLS_RESPONSE=$( (echo "$SEQUENCE"; sleep 0.5) | "$SERVER_BIN" 2>/dev/null | grep '"method":"tools/list"' || echo "$SEQUENCE" | "$SERVER_BIN" 2>/dev/null | grep 'write_file' | head -1)
log_info "Tools list response: $TOOLS_RESPONSE"

if echo "$TOOLS_RESPONSE" | grep -q 'write_file'; then
    log_info "✓ write_file tool found"
else
    log_error "✗ write_file tool not found"
    exit 1
fi

# Test 3: Call write_file tool with valid absolute path
log_test "Test 3: Call write_file with valid absolute path"
WRITE_REQUEST=$(cat <<EOF
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"write_file","arguments":{"path":"$TEST_FILE","content":"Hello from rmcp test!"}}}
EOF
)

FULL_SEQUENCE=$(cat <<EOF
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{"roots":{"listChanged":true}},"clientInfo":{"name":"test-harness","version":"1.0.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}
$WRITE_REQUEST
EOF
)

WRITE_RESPONSE=$( (echo "$FULL_SEQUENCE"; sleep 1) | "$SERVER_BIN" 2>/dev/null | grep '"id":3' || true)
log_info "Write response: $WRITE_RESPONSE"

# Verify the file was created
if [ -f "$TEST_FILE" ]; then
    CONTENT=$(cat "$TEST_FILE")
    if [ "$CONTENT" = "Hello from rmcp test!" ]; then
        log_info "✓ File written successfully with correct content"
    else
        log_error "✗ File content mismatch. Expected 'Hello from rmcp test!', got '$CONTENT'"
        exit 1
    fi
else
    log_error "✗ File was not created at $TEST_FILE"
    exit 1
fi

# Test 4: Call write_file with relative path (should fail)
log_test "Test 4: Call write_file with relative path (should fail)"
RELATIVE_REQUEST=$(cat <<EOF
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"write_file","arguments":{"path":"relative/path.txt","content":"This should fail"}}}
EOF
)

RELATIVE_SEQUENCE=$(cat <<EOF
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{"roots":{"listChanged":true}},"clientInfo":{"name":"test-harness","version":"1.0.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}
$RELATIVE_REQUEST
EOF
)

RELATIVE_RESPONSE=$( (echo "$RELATIVE_SEQUENCE"; sleep 1) | "$SERVER_BIN" 2>/dev/null | grep '"id":4' || true)
log_info "Relative path response: $RELATIVE_RESPONSE"

if echo "$RELATIVE_RESPONSE" | grep -q '"error"'; then
    log_info "✓ Relative path correctly rejected"
else
    log_error "✗ Relative path should have been rejected"
    exit 1
fi

# Summary
echo ""
log_info "========================================="
log_info "All tests passed! ✓"
log_info "========================================="
log_info "Summary:"
log_info "  - MCP initialization: ✓"
log_info "  - Tool discovery: ✓"
log_info "  - File write (absolute path): ✓"
log_info "  - File write (relative path rejected): ✓"
