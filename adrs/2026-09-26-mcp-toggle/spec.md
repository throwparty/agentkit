# Toggling of Configured MCP Server Availability

**Status:** implemented  **Created:** 2026-09-26  **Author:** Luke Carrier

The tackle agent currently connects to all configured MCP servers automatically and maintains persistent connections. There is no mechanism to temporarily disable or re-enable individual MCP servers without modifying configuration files and restarting the agent. This limits operational flexibility for maintenance, troubleshooting, or resource management.


## Problem

Users cannot selectively enable or disable configured MCP servers at runtime, requiring configuration changes and agent restarts to manage MCP server availability. Users need an intuitive command-based interface to control MCP server availability that works across all ACP clients.

## Goals

- Add runtime capability to enable/disable individual MCP servers
- Maintain connection state when servers are re-enabled
- Provide intuitive command-based user interface for MCP server management


## Functional Requirements

### FR-001: MCP Server Toggle Command

Users can disable individual configured MCP servers via command interface (e.g. /mcp disable <server-id>)

**Slug:** `mcp-server-toggle-command`

### FR-002: MCP Server Re-enable Command

Users can re-enable previously disabled MCP servers via command interface (e.g. /mcp enable <server-id>)

**Slug:** `mcp-server-reenable-command`

### FR-003: Toggle Status Query Command

Users can query the enabled/disabled status of MCP servers via command interface (e.g. /mcp status [<server-id>])

**Slug:** `mcp-toggle-status-command`

### FR-004: Automatic Reconnection

Re-enabled servers automatically attempt to reconnect

**Slug:** `mcp-auto-reconnect`

## Non-functional Requirements

### NFR-001: Backward Compatibility

Existing behavior unchanged when feature not used

**Slug:** `backward-compatibility`

### NFR-002: State Persistence

Connection state preserved when toggling servers

**Slug:** `state-persistence`

## Acceptance Criteria

### AC-001: Disable Server

Disabling a server prevents new connections and closes existing ones

**Slug:** `disable-server`

### AC-002: Enable Server

Enabling a server initiates connection attempts

**Slug:** `enable-server`

### AC-003: Status Reporting

Toggle status is accurately reported in MCP pool statuses

**Slug:** `status-reporting`

### AC-004: Connection Preservation

Re-enabled servers attempt to restore previous connection state

**Slug:** `connection-preservation`

## Edge Cases

### EC-001: Already Disabled Server

Attempting to disable an already-disabled server is a no-op

**Slug:** `already-disabled`
### EC-002: Failed Reconnection

Re-enabling a server that fails to connect reports the failure appropriately

**Slug:** `failed-reconnect`

