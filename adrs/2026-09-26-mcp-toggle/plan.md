# Toggling of Configured MCP Server Availability - Technical Plan

## Approach

Modify the McpPool struct to track enabled/disabled state per server, update connection logic to respect this state, and add ACP command handlers for /mcp disable/enable/status commands.

## Architecture

The solution extends the McpPool struct in src/mcp/mod.rs to include an enabled flag per server, modifies the connect method to skip disabled servers, adds public methods to toggle server state, and implements ACP command handlers in src/acp/mod.rs to expose the functionality via /mcp commands.

## Technologies

| Technology | Role |
|------------|------|
| agentkit-tackle | MCP server management |
| agentkit-tackle__acp | ACP command handling |
| agentkit-tackle__mcp | MCP pool and connection management |
| agentkit-tackle__store | Session state persistence |

## Components

### McpPool_State_Extension

Add enabled/disabled tracking to McpPool


### Connection_Logic_Update

Modify connect method to respect enabled state


### Public_Toggle_Methods

Add enable/disable/status methods to McpPool


### ACP_Command_Handler

Handle /mcp disable/enable/status commands


## Data Flow

1. User sends /mcp disable server1 via ACP; 2. ACP PromptRequest handler detects slash command and calls mcp_pool.disable_server("server1"); 3. McpPool updates enabled state and closes existing connection if any; 4. Subsequent agent turns see server as disabled and don't attempt reconnection; 5. User sends /mcp enable server1; 6. ACP PromptRequest handler detects slash command and calls mcp_pool.enable_server("server1"); 7. McpPool updates enabled state and attempts reconnection if previously connected; 8. User sends /mcp status; 9. ACP PromptRequest handler calls mcp_pool.statuses() and enabled states to report comprehensive server status


## Deployment

No special deployment considerations - this is a pure Rust library change that affects the tackle binary. No database schema changes or configuration updates required. All changes are backward compatible - existing behavior preserved when feature not used.

