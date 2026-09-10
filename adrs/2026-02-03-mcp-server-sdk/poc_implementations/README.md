# MCP Server SDK PoC Test Harnesses

This directory contains test harnesses for evaluating MCP server SDK implementations.

## Test Harnesses

### Python Test Client (`test_mcp_server.py`)

A comprehensive test client that sends JSON-RPC messages to the MCP server over stdio.

**Usage:**

```bash
python3 test_mcp_server.py <poc-directory>
```

**Example:**

```bash
python3 test_mcp_server.py poc-rmcp
```

**Tests performed:**

1. Initialize MCP connection
1. List available tools
1. Call `write_file` with absolute path (should succeed)
1. Call `write_file` with relative path (should fail)

### Bash Test Script (`test_mcp_server.sh`)

A shell-based test harness for environments without Python.

**Usage:**

```bash
./test_mcp_server.sh <poc-directory>
```

**Example:**

```bash
./test_mcp_server.sh poc-rmcp
```

## MCP Protocol Messages

The test harnesses send these JSON-RPC messages:

### 1. Initialize

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": "2024-11-05",
    "capabilities": {
      "roots": {
        "listChanged": true
      }
    },
    "clientInfo": {
      "name": "test-harness",
      "version": "1.0.0"
    }
  }
}
```

### 2. Initialized Notification

```json
{
  "jsonrpc": "2.0",
  "method": "notifications/initialized",
  "params": {}
}
```

### 3. List Tools

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/list",
  "params": {}
}
```

### 4. Call write_file (Absolute Path)

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/call",
  "params": {
    "name": "write_file",
    "arguments": {
      "path": "/tmp/test.txt",
      "content": "Hello MCP!"
    }
  }
}
```

### 5. Call write_file (Relative Path - Should Fail)

```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "method": "tools/call",
  "params": {
    "name": "write_file",
    "arguments": {
      "path": "relative/path.txt",
      "content": "Should fail"
    }
  }
}
```

## Expected Responses

### Successful Initialize Response

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "protocolVersion": "2024-11-05",
    "capabilities": {
      "tools": {}
    },
    "serverInfo": {
      "name": "...",
      "version": "..."
    }
  }
}
```

### Successful write_file Response

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {
    "content": [
      {
        "type": "text",
        "text": "Successfully wrote 10 bytes to /tmp/test.txt"
      }
    ]
  }
}
```

### Failed write_file Response (Relative Path)

```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "error": {
    "code": -32602,
    "message": "path must be absolute"
  }
}
```

## PoC Structure

Each PoC implementation should:

1. Be in a directory named `poc-<sdk-name>`
1. Be a valid Rust cargo project
1. Build a binary with the same name as the directory (e.g., `poc-rmcp` builds `target/debug/poc-rmcp`)
1. Implement the `write_file` tool as specified in the ADR
1. Accept JSON-RPC messages on stdin and respond on stdout
