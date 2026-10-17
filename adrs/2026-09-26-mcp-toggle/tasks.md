# Toggling of Configured MCP Server Availability - Implementation Tasks

## Tasks

### T-001: [x] Add enabled state tracking to McpPool

Add enabled: BTreeMap<String, bool> field to McpPool struct and initialize all configured servers as enabled: true in constructor


| Field | Value |
|-------|-------|
| Success Criteria | McpPool struct has enabled field, all servers initialized as enabled |
| Complexity | 🟢 Low |
| Effort | 2h |
| Depends On |  |
| References | FR-001,FR-002,FR-003,FR-004 |

### T-002: [x] Modify McpPool connection logic

Update McpPool::connect method to skip servers where enabled.get(server) == Some(false)


| Field | Value |
|-------|-------|
| Success Criteria | McpPool::connect respects enabled state and skips disabled servers |
| Complexity | 🟢 Low |
| Effort | 1h |
| Depends On | T-001 |
| References | FR-001,FR-002 |

### T-003: [x] Add public toggle methods to McpPool

Implement McpPool::enable_server, disable_server, is_enabled, and toggle_server methods


| Field | Value |
|-------|-------|
| Success Criteria | All public methods compile and function correctly |
| Complexity | 🟡 Medium |
| Effort | 3h |
| Depends On | T-001 |
| References | FR-001,FR-002,FR-003,FR-004 |

### T-004: [x] Handle connection lifecycle on toggle

When disabling a server with active connection: close the connection; when enabling a previously connected server: attempt reconnection


| Field | Value |
|-------|-------|
| Success Criteria | Disabling closes existing connections, enabling attempts reconnection |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-003 |
| References | FR-001,FR-002,FR-004 |

### T-005: [x] Update McpPool status reporting

Modify McpPool::statuses method to include enabled/disabled state in returned status


| Field | Value |
|-------|-------|
| Success Criteria | Status reporting includes enabled/disabled information alongside connection status |
| Complexity | 🟢 Low |
| Effort | 1h |
| Depends On | T-003 |
| References | FR-003 |

### T-006: [x] Add ACP command handler detection

In acp::run_agent_over PromptRequest handler, add condition to detect /mcp slash commands


| Field | Value |
|-------|-------|
| Success Criteria | PromptRequest handler correctly identifies /mcp commands |
| Complexity | 🟢 Low |
| Effort | 1h |
| Depends On |  |
| References | FR-001,FR-002,FR-003 |

### T-007: [x] Implement ACP /mcp disable command

Handle /mcp disable <server> by calling mcp_pool.disable_server(server) and returning appropriate response


| Field | Value |
|-------|-------|
| Success Criteria | /mcp disable command works and returns success/error messages |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-006 |
| References | FR-001 |

### T-008: [x] Implement ACP /mcp enable and status commands

Handle /mcp enable <server> and /mcp status [<server-id>] by calling appropriate McpPool methods


| Field | Value |
|-------|-------|
| Success Criteria | /mcp enable and status commands work correctly |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-006 |
| References | FR-002,FR-003 |

