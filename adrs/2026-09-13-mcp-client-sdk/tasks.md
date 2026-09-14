# MCP Client SDK: evaluation tasks

## Tasks

### T-001: [x] Build the shared reference MCP server

Create poc_implementations/mcp-server: a Rust crate on the already-adopted server SDK (rmcp 3.3.0, features macros, schemars, server, transport-io) exposing an echo tool over stdio and implementing the current protocol era (2026-07-28 via server/discover) alongside the legacy initialize handshake. Both client PoCs must spawn this one binary - the server implementation is shared, never duplicated per client. The crate also exposes a helper (lib) that locates/builds the binary so both PoC test suites spawn the same server.

| Field            | Value                                                                                                                                              |
| ---------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| Success Criteria | cargo build -p mcp-server succeeds; the binary completes a client handshake over the current protocol era; the helper returns a spawnable command. |
| Complexity       | 🟡 Medium                                                                                                                                          |
| Effort           | 2-3h                                                                                                                                               |
| Depends On       |                                                                                                                                                    |
| References       | stdio-transport, initialization-handshake, tool-discovery, tool-invocation, stdio-poc, current-protocol-era                                        |

### T-002: Build rmcp stdio PoC

Create poc_implementations/poc-rmcp: a Rust crate using rmcp 3.3.0 with features client and transport-child-process. Spawn the shared mcp-server via TokioChildProcess (explicit argv, no shell), complete the handshake over the current protocol era (server/discover), call list_all_tools(), then call the echo tool with a JSON message. Print tool count, first five tool names, and the tool result in a fixed output shape.

| Field            | Value                                                                                                                                                       |
| ---------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Success Criteria | cargo run succeeds; the crate prints a handshake-complete marker, a tool count greater than zero, the first five tool names, and the echo tool text result. |
| Complexity       | 🟡 Medium                                                                                                                                                   |
| Effort           | 2-4h                                                                                                                                                        |
| Depends On       | T-001                                                                                                                                                       |
| References       | stdio-transport, initialization-handshake, tool-discovery, tool-invocation, stdio-poc                                                                       |

### T-003: Build rust-mcp-sdk stdio PoC

Create poc_implementations/poc-rust-mcp-sdk: a Rust crate using rust-mcp-sdk 2.0.0 with default-features disabled and features client and stdio. Spawn the same shared mcp-server via StdioTransport create_with_server_launch with a command string and explicit argv array, create the client runtime with ClientDetails, start it, call request_tool_list(), then call the echo tool. Print the same output shape as T-002.

| Field            | Value                                                                                                                                                |
| ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| Success Criteria | cargo run succeeds; output matches the T-002 shape (handshake marker, tool count, first five names, echo result) against the same shared mcp-server. |
| Complexity       | 🟡 Medium                                                                                                                                            |
| Effort           | 3-5h                                                                                                                                                 |
| Depends On       | T-001                                                                                                                                                |
| References       | stdio-transport, initialization-handshake, tool-discovery, tool-invocation, stdio-poc                                                                |

### T-004: Run and compare both PoCs

Add a PoC runner (script or Makefile) that executes T-002 and T-003 against the same shared mcp-server and captures their stdout side by side. Verify both complete the full flow (handshake, tool list, tool call). Record the runs and any failures in the ADR comparison notes in spec.toon's Evaluation section, including the protocol-era finding: the TS reference server-everything implements only the legacy 2025-11-25 initialize handshake, so a 2026-07-28-only client SDK cannot connect to it; the shared mcp-server implements both eras so the comparison is apples-to-apples.

| Field            | Value                                                                                                                                                                       |
| ---------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Success Criteria | Both PoCs complete against the same server; differences and failures are recorded in the Evaluation section; if either PoC fails, the failure and its cause are documented. |
| Complexity       | 🟢 Low                                                                                                                                                                      |
| Effort           | 1-2h                                                                                                                                                                        |
| Depends On       | T-002, T-003                                                                                                                                                                |
| References       | candidate-comparison, stdio-poc, current-protocol-era                                                                                                                       |

### T-005: Select the library and record the decision

Decide the winning library by comparing the PoC runs against the spec: NFR-001 consistency with the adopted rmcp server SDK, NFR-002 crates.io stability, both transports, conformance, and shared protocol types. Record the winner and the rationale in spec.toon's Evaluation section. If the winner is not rmcp (NFR-001/AC-005 conflict), amend the spec's acceptance criteria and non-functional requirements to match reality (EC-005). Move the ADR status to accepted.

| Field            | Value                                                                                                                                                             |
| ---------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Success Criteria | spec.toon Evaluation names the winner with rationale; ADR status is accepted; requirement slugs referenced here are marked satisfied in the spec's current state. |
| Complexity       | 🟢 Low                                                                                                                                                            |
| Effort           | 1h                                                                                                                                                                |
| Depends On       | T-004                                                                                                                                                             |
| References       | client-sdk-selected, consistency-with-existing-choice, stable-release, maintenance, ecosystem-fit, same-crate-as-server, fallback-path                            |

### T-006: Extend the winner to streamable HTTP

Extend the winning PoC crate with a second mode that connects over streamable HTTP (rmcp transport-streamable-http-client-reqwest or rust-mcp-sdk streamable-http feature) to a remote MCP server - run mcp-server's HTTP transport or another reachable server - then lists tools and calls one tool.

| Field            | Value                                                                                                                                                |
| ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| Success Criteria | The extended PoC connects over streamable HTTP with TLS, lists tools, and calls one tool successfully; result is recorded in the Evaluation section. |
| Complexity       | 🟡 Medium                                                                                                                                            |
| Effort           | 2-3h                                                                                                                                                 |
| Depends On       | T-005                                                                                                                                                |
| References       | streamable-http-transport, http-poc                                                                                                                  |

### T-007: Wire per-session MCP client lifecycle into the harness

In the harness (the ACP server implementation), add the chosen SDK as a dependency and implement the per-session client lifecycle (plan component: Harness MCP client module): for each MCP server in session/new mcpServers, connect - stdio entries spawned with explicit argv from the configured command and args, never a shell string, with only per-server configured env (EC-006), remote entries via the streamable HTTP client with verified TLS by default (NFR-005) - complete the handshake with clear errors on version mismatch, and disconnect on session/close, surfacing server process exits and unreachable remote servers as errors (no panics, empty ok on missing capabilities).

| Field            | Value                                                                                                                                                                                                                                               |
| ---------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Success Criteria | Session lifecycle works end to end against a real server: connect on session/new, handshake succeeds, disconnect on session/close kills stdio children; process exit, version mismatch, and unreachable server cases surface as errors, not panics. |
| Complexity       | 🔴 High                                                                                                                                                                                                                                             |
| Effort           | 6-9h                                                                                                                                                                                                                                                |
| Depends On       | T-005                                                                                                                                                                                                                                               |
| References       | stdio-transport, streamable-http-transport, initialization-handshake, shared-protocol-implementation, untrusted-spawn-command, secure-outbound-defaults, server-process-exit, protocol-version-mismatch, remote-server-unreachable                  |

### T-008: Expose tools from connected servers

On top of T-007, expose the discovered tools of each connected server: list tools per server with names, descriptions, and input schemas, call tools by name with JSON arguments, and return structured results including tool-level and protocol-level errors. A server advertising no tools yields an empty list, not an error. The harness uses the SDK directly (FR-006) with no wrapper abstraction.

| Field            | Value                                                                                                                                                                                    |
| ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Success Criteria | The harness can list every configured server's tools and call a tool with JSON arguments, returning structured results; a toy server with no tools produces an empty list without panic. |
| Complexity       | 🟡 Medium                                                                                                                                                                                |
| Effort           | 3-5h                                                                                                                                                                                     |
| Depends On       | T-007                                                                                                                                                                                    |
| References       | tool-discovery, tool-invocation, capability-gap, shared-protocol-implementation                                                                                                          |
