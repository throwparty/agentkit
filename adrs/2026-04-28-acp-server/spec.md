---
status: draft
created: 2026-04-28
updated: 2026-04-29
author: adrian
decision: pending
---

# ACP Server Specification

## Status Update
- **Updated**: 2026-06-08

## Problem

We need a basic harness for an AI agent using the Agent Client Protocol (ACP). The server should return static responses to validate our understanding of the protocol and provide a testable interface for development and experimentation.

**Scope**: This is not a production service. No production concerns apply (expiry, limits, persistence, multi-client). Sessions are stored in a simple `HashMap` and live in memory until the server stops.

## Goals

- Implement an ACP-compliant server supporting two transport layers (stdio and HTTP)
- Return deterministic static responses for all received messages
- Provide a simple CLI to select transport at startup
- Include a test suite validating core ACP message handling
- Support integration testing via Zed as a third-party client

## User Journey

1. User starts the ACP server with `acp-server --transport stdio|http`
2. A client (e.g., Zed) connects via the selected transport
3. Client sends `initialize` request
4. Server responds with capabilities and protocol version
5. Client creates a session via `session/new`
6. Client sends `session/prompt` with user message
7. Server sends `session/update` notification echoing the user message, then responds with `stopReason: "end_turn"`
8. Client receives notifications, displays content, and processes the final response
9. Client optionally closes the session

## Requirements

### Transport Options

| Transport | Endpoint | Behavior |
|-----------|----------|----------|
| `stdio` | stdin/stdout | Line-delimited JSON messages, single client, server blocks on read/write |
| `http` | `{--bind}:{--http-port}` (default 3811) | Streamable HTTP with JSON-RPC, JSON request/response |

**Port Configuration:**
- `--bind` (default `127.0.0.1`) - network interface to bind
- `--http-port` (default 3811) - port for HTTP transport

**Functional Requirements:**
- `--transport` flag is required at startup; error if omitted or invalid
- Each transport runs a single server instance
- Server must handle graceful shutdown on SIGINT/SIGTERM

### Message Format (JSON-RPC 2.0 per ACP)

ACP messages use JSON-RPC 2.0 encoding. All messages must be UTF-8 encoded.

**Request/Response Structure:**
```json
{
  "jsonrpc": "2.0",
  "id": "string (message ID)",
  "method": "string (method name)",
  "params": { "key": "value" }
}
```

**Response Structure:**
```json
{
  "jsonrpc": "2.0",
  "id": "string (matching request ID)",
  "result": { ... }
}
```

**Error Structure:**
```json
{
  "jsonrpc": "2.0",
  "id": "string (matching request ID)",
  "error": {
    "code": number (JSON-RPC error code),
    "message": "string"
  }
}
```

**Notification Structure:**
```json
{
  "jsonrpc": "2.0",
  "method": "string (method name)",
  "params": { ... }
}
```

**References:**
- ACP Transports: Messages are UTF-8 encoded JSON-RPC 2.0 messages
- ACP Overview: Protocol uses JSON-RPC 2.0 with two message types (requests and notifications)
- ACP Schema: All method-specific request/response structures defined

**Acceptance Criteria:**
- All messages include `"jsonrpc": "2.0"` per ACP Transports
- Request IDs match between request and response per JSON-RPC 2.0
- Notifications have no `id` field per JSON-RPC 2.0
- Invalid JSON-RPC format returns parse error per JSON-RPC 2.0 spec

### Initialization Phase (ACP Lifecycle)

The connection must begin with the `initialize` method before any session can be created.

**`initialize` Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": 1,
    "clientCapabilities": {
      "fs": {
        "readTextFile": boolean,
        "writeTextFile": boolean
      },
      "terminal": boolean
    },
    "clientInfo": {
      "name": string,
      "title": string,
      "version": string
    }
  }
}
```

**`initialize` Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "protocolVersion": 1,
    "agentCapabilities": {
      "loadSession": false,
      "promptCapabilities": {
        "image": false,
        "audio": false,
        "embeddedContext": false
      },
      "mcpCapabilities": {
        "http": false,
        "sse": false
      },
      "sessionCapabilities": {
        "list": {},
        "close": {},
        "resume": {}
      }
    },
    "agentInfo": {
      "name": "acp-server",
      "title": "ACP Server Harness",
      "version": "0.1.0"
    },
    "authMethods": []
  }
}
```

**References:**
- ACP Initialization: Client MUST call `initialize` before creating sessions
- ACP Initialization: Agent MUST respond with chosen protocol version and capabilities
- ACP Initialization: `sessionCapabilities` contains nested capability objects for `list`, `close`, `resume`
- ACP Schema: InitializeRequest and InitializeResponse structures

**Acceptance Criteria:**
- Server responds with protocol version 1 and capabilities
- Server reports `loadSession: false` (basic harness does not replay conversation history)
- Server reports `sessionCapabilities.list: {}` (supports `session/list`)
- Server reports `sessionCapabilities.close: {}` (supports `session/close`)
- Server reports `sessionCapabilities.resume: {}` (supports `session/resume`)
- Server reports empty `authMethods` (no auth required)
- Server reports no prompt/media capabilities (text-only echo)

#### `authenticate` (ACP Lifecycle)

Client sends authentication credentials. Not required for this harness (empty `authMethods` in initialize response).

**`authenticate` Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "authenticate",
  "params": {
    "authMethod": string
  }
}
```

**References:**
- ACP Authentication: Agent advertises `authMethods` in initialize response
- ACP Authentication: Client calls `authenticate` only if a non-empty `authMethods` array is present

**Acceptance Criteria:**
- Not implemented; returns error `-32601 Method not found` (client should not call if `authMethods` is empty)

### Session Management

Sessions are stored in a simple in-memory `HashMap<SessionId, Session>`. No persistence, no expiry, no limits.

#### `session/new` (ACP Lifecycle)

Clients create a new session by calling the `session/new` method.

**`session/new` Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "session/new",
  "params": {
    "cwd": string,
    "mcpServers": []
  }
}
```

**`session/new` Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "sessionId": "string (UUIDv4)",
    "modes": {
      "currentModeId": "ask",
      "availableModes": [
        {
          "id": "ask",
          "name": "Ask",
          "description": "Request permission before making any changes"
        }
      ]
    }
  }
}
```

**References:**
- ACP Session Setup: Clients MUST complete initialization before creating sessions
- ACP Session Setup: Client sends `cwd` and optional `mcpServers`
- ACP Session Setup: Agent responds with `sessionId` and optional `modes`
- ACP Session Modes: Agent MAY return `modes` in response including `currentModeId` and `availableModes`

**Acceptance Criteria:**
- Server creates new session (UUIDv4) and returns session ID
- Server returns `modes` with current mode and available modes
- `cwd` is stored but not validated (basic harness)
- `mcpServers` is ignored (no MCP integration)

#### `session/load` (ACP Lifecycle)

Agents that support the `loadSession` capability allow Clients to restore previous conversations by replaying history.

**`session/load` Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "session/load",
  "params": {
    "sessionId": string,
    "cwd": string,
    "mcpServers": []
  }
}
```

**`session/load` Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32602,
    "message": "loadSession capability is not supported"
  }
}
```

**References:**
- ACP Session Setup: Client MUST verify `loadSession` capability before attempting load
- ACP Session Setup: Agent replays conversation history as `session/update` notifications, then responds

**Acceptance Criteria:**
- Server returns error (loadSession capability is false)
- Basic harness does NOT support session loading (returns error -32602)

#### `session/resume` (ACP Lifecycle)

Agents that support the `sessionCapabilities.resume` capability allow Clients to reconnect to an existing session without replaying conversation history.

**`session/resume` Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "session/resume",
  "params": {
    "sessionId": string,
    "cwd": string,
    "mcpServers": []
  }
}
```

**`session/resume` flow:**
1. Server receives `session/resume` with `sessionId`
2. Server replays dummy conversation history as `session/update` notifications (user_message_chunk + agent_message_chunk)
3. Server persists dummy messages to in-memory session store
4. Server returns `{}`

**`session/resume` Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {}
}
```

**`session/update` Notification (sent before response):**
```json
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": string,
    "update": {
      "sessionUpdate": "user_message_chunk",
      "content": {
        "type": "text",
        "text": string
      }
    }
  }
}
```

**References:**
- ACP Session Setup: Client MUST verify `sessionCapabilities.resume` before attempting resume
- ACP Session Setup: Agent restores session context and reconnects to MCP servers, returns once ready (does NOT replay history)

**Acceptance Criteria:**
- Server returns empty result `{}` unconditionally (dummy resume — no persistent state)
- Server replays dummy conversation as one or more `session/update` notifications before responding
- Dummy messages are stored in-memory and compound across resumes within the same process lifetime

#### `session/list` (ACP Lifecycle)

**`session/list` Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "session/list",
  "params": {
    "cwd": string,
    "cursor": string
  }
}
```

**`session/list` Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {
    "sessions": [
      {
        "sessionId": string,
        "cwd": string,
        "title": string,
        "updatedAt": string (ISO 8601),
        "_meta": {
          "messageCount": number,
          "hasErrors": boolean
        }
      }
    ],
    "nextCursor": string
  }
}
```

**References:**
- ACP Session List: Clients MUST verify `sessionCapabilities.list` capability before calling
- ACP Session List: Response includes `sessions` array and optional `nextCursor` for pagination

**Acceptance Criteria:**
- Server returns list of all sessions (no pagination implemented)
- `nextCursor` is absent (no more results)
- Sessions include `sessionId`, `cwd`, `title`, `updatedAt`, `_meta`

#### `session/close` (ACP Lifecycle)

**`session/close` Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "method": "session/close",
  "params": {
    "sessionId": string
  }
}
```

**`session/close` Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "result": {}
}
```

**References:**
- ACP Session Close: Clients MUST verify `sessionCapabilities.close` capability before calling
- ACP Session Close: Agent cancels ongoing work and frees resources

**Acceptance Criteria:**
- Server removes session from the in-memory map
- Returns empty result `{}`
- Errors gracefully if session doesn't exist

#### `session/set_mode` (ACP Lifecycle)

Client requests a mode change for a session.

**`session/set_mode` Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 5,
  "method": "session/set_mode",
  "params": {
    "sessionId": string,
    "mode": string
  }
}
```

**References:**
- ACP Session Modes: Client may request mode changes during session lifecycle
- ACP Session Modes: Mode change notification `current_mode_update` is sent via `session/update` when mode changes

**Acceptance Criteria:**
- Server stores the mode on the session object
- Returns `{}`
- Session existence is validated before processing

#### `session/set_config_option` (ACP Lifecycle)

Client requests a configuration change for a session.

**`session/set_config_option` Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 6,
  "method": "session/set_config_option",
  "params": {
    "sessionId": string,
    "option": string,
    "value": any
  }
}
```

**References:**
- ACP Session Config: Client may request config changes during session lifecycle

**Acceptance Criteria:**
- Not implemented; returns error `-32601 Method not found` (config options are out of scope for text echo harness)

### Prompt Turn (ACP Lifecycle)

#### `session/prompt` (Core Feature)

The core conversation flow. Client sends a user message, agent responds.

**`session/prompt` Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 5,
  "method": "session/prompt",
  "params": {
    "sessionId": string,
    "prompt": [
      {
        "type": "text",
        "text": "string"
      }
    ]
  }
}
```

**`session/prompt` Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 5,
  "result": {
    "stopReason": string (e.g., "end_turn", "cancelled")
  }
}
```

**`session/update` Notification (from agent to client):**
```json
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": string,
    "update": {
      "sessionUpdate": "agent_message_chunk",
      "content": {
        "type": "text",
        "text": "string"
      }
    }
  }
}
```

**References:**
- ACP Prompt Turn: Core interaction cycle from user message to agent response
- ACP Prompt Turn: Agent sends `session/update` notifications during processing
- ACP Prompt Turn: Turn ends with `session/prompt` response containing `stopReason`
- ACP Session Modes: `current_mode_update` sent via `session/update` when mode changes

**Acceptance Criteria:**
- Server sends `session/update` notification with `agent_message_chunk` echoing the user's text
- Server responds with `stopReason: "end_turn"`
- Session existence is validated before processing (returns error if missing)

#### `session/cancel` (Notification)

Client sends notification to cancel ongoing operations.

**`session/cancel` Notification:**
```json
{
  "jsonrpc": "2.0",
  "method": "session/cancel",
  "params": {
    "sessionId": string
  }
}
```

**References:**
- ACP Prompt Turn Cancellation: Client sends notification (no `id`, no response expected for cancel itself)
- ACP Prompt Turn Cancellation: Agent MUST respond to original `session/prompt` with `stopReason: "cancelled"`

**Acceptance Criteria:**
- Server processes cancellation notification (no response to cancel itself)
- Server responds to the original pending `session/prompt` with `stopReason: "cancelled"`
- No `session/update` notification sent for cancellation (the cancel notification itself carries the signal)

### Static Response Behavior

| Method | Response Type | Behavior |
|--------|--------------|----------|
| `initialize` | response | Static capabilities and protocol version |
| `authenticate` | error | Return `-32601 Method not found` (no auth methods advertised) |
| `session/new` | response | Return unique session ID and modes |
| `session/load` | error | Return error (loadSession capability false) |
| `session/resume` | response | Return empty result `{}` (session restored) |
| `session/list` | response | Return list of active sessions |
| `session/close` | response | Close session, return `{}` |
| `session/set_mode` | response | Store mode on session, return `{}` |
| `session/set_config_option` | error | Return `-32601 Method not found` (config out of scope) |
| `session/prompt` | response + notification | Echo user message, respond with `end_turn` |
| `session/cancel` | notification | Process cancellation, respond to original prompt |
| unknown | error | Return `-32601 Method not found` |

**Acceptance Criteria:**
- Responses are deterministic except for session IDs
- Every supported method has a documented, reproducible behavior
- Unsupported methods return `-32601 Method not found`
- JSON-RPC 2.0 compliance (valid `jsonrpc` field, proper `id` handling)
- `session/set_config_option` is not implemented (config out of scope for text-echo harness)

### ACP Method Reference

All methods defined by the ACP spec (from `acp-spec/schema.mdx`). Methods marked "agent" are handled by the server; methods marked "client" are called by the agent on the client (not implemented in this harness).

| Method | Direction | Status | Notes |
|--------|-----------|--------|-------|
| `initialize` | agent → client | ✅ implemented | Protocol version negotiation, capability exchange |
| `authenticate` | agent → client | ❌ not implemented | Auth not required (empty `authMethods`) |
| `session/new` | agent → client | ✅ implemented | Creates session, returns UUID |
| `session/load` | agent → client | ❌ not implemented | Returns error `-32602` (loadSession capability is false) |
| `session/resume` | agent → client | ❌ not implemented | Returns `{}` if session exists, error if not |
| `session/list` | agent → client | ❌ not implemented | Returns list of active sessions |
| `session/close` | agent → client | ✅ implemented | Closes session, returns `{}` |
| `session/prompt` | agent → client | ✅ implemented | Echoes user message, returns `end_turn` |
| `session/cancel` | agent → client | ❌ not implemented | Notification; responds to pending prompt with `cancelled` |
| `session/set_mode` | agent → client | ❌ not implemented | Changes session mode |
| `session/set_config_option` | agent → client | ❌ not implemented | Changes session config |
| `session/update` | agent → client | ✅ implemented | Notification streamed to client during prompt turn |
| `session/request_permission` | client → agent | ❌ not implemented | Agent requests user permission for tool calls |
| `fs/read_text_file` | client → agent | ❌ not implemented | Client-side file read |
| `fs/write_text_file` | client → agent | ❌ not implemented | Client-side file write |
| `terminal/*` | client → agent | ❌ not implemented | Client-side terminal management |
