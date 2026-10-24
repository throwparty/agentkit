# Tackle: ACP v2 Server Support — Tasks

## Tasks

### T-001: [x] Conformance runner per-version baselines

Parameterise the conformance runner in crates/agentkit-tackle/mod.just by protocol version (it hardcodes --protocol-version 1 today) and key the baseline by version so v1 and v2 have separate committed baselines. Rework tackle-conformance-check to take the version or a per-version baseline path. Wire the v2 run into the existing CI job.


| Field | Value |
|-------|-------|
| Success Criteria | just tackle conformance 1 and just tackle conformance 2 both run to completion and diff against their own baseline; the v2 run reports a VERSION-MISMATCH as failure rather than a pass when the agent does not negotiate v2; the CI job runs both. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On |  |
| References | conformance-runner-coverage,protocol-conformance |

### T-002: [x] Extract acp::common

Lift the protocol-neutral orchestration out of acp_v1 into a new acp::common module: ids, the shared MCP pool connection state, the version-neutral prompt dispatch decision, lease and prompt-turn persistence, the domain command catalogue, actor/model and config-option resolution, the replay DAG walk to a NeutralEvent stream, the /context report assembly, and assistant-message persistence. No behaviour change.


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests cover the neutral seams (dispatch decision over first-user-text inputs with the v1 guardrails, NeutralEvent ordering, id mapping); acp_v1 compiles against acp::common with no behaviour change; the v1 test suites pass unchanged. |
| Complexity | 🔴 High |
| Effort | 6-8h |
| Depends On |  |
| References | shared-core-boundary,protocol-neutral-core |

### T-003: [x] Relocate acp::v1 and acp::transport

Move the existing acp_v1 module to acp::v1 with no logic change, and move the stdio and axum HTTP transports out of it into acp::transport so they are no longer version-scoped. Re-point every call site.


| Field | Value |
|-------|-------|
| Success Criteria | acp::v1 holds the former acp_v1 contents; acp::transport holds run_stdio and the HTTP transport; the full v1 test suite and the v1 acp-tck conformance run still pass. |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On | T-002 |
| References | shared-core-boundary,v1-preservation |

### T-004: [x] Version-negotiating entry point

Add run_acp_over in acp::mod, built on Agent.protocol_router(), for now configured with the v1 implementation only. The stdio and HTTP transports call it instead of the v1 builder. The router owns version selection and the -32022 rejection.


| Field | Value |
|-------|-------|
| Success Criteria | A v1 client completes a session lifecycle through run_acp_over over stdio and over HTTP; a client requesting an unsupported version is rejected with -32022; no version-selection logic exists outside the router. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-003 |
| References | version-negotiation,shared-core-boundary |

### T-005: arch-lint boundary gate

Add arch-lint.toml declaring the acp::common / acp::v1 / acp::v2 layering and the sibling prohibition (acp::v1 and acp::v2 must not import each other), and wire it into the existing CI gate.


| Field | Value |
|-------|-------|
| Success Criteria | arch-lint runs in CI; a deliberate import from acp::v1 into acp::v2 fails the check; the clean tree passes. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-003 |
| References | module-boundary-enforcement,shared-core-boundary |

### T-006: v1 conformance preservation gate

Run the v1 acp-tck suite after the refactor and confirm it still reports CONFORMANT with the empty MANDATORY baseline. Fix any regression the restructure introduced. This is the inertness proof for the refactor and the gate before v2 work begins.


| Field | Value |
|-------|-------|
| Success Criteria | just tackle conformance 1 reports CONFORMANT; the committed v1 baseline is unchanged (empty); the full cargo test -p agentkit-tackle suite passes. |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On | T-001,T-004,T-005 |
| References | v1-conformance-preserved,protocol-conformance,v1-preservation |

### T-007: v2 module skeleton, capabilities and auth

Add acp::v2 with its ConnectTo builder, capability map (object support markers, session-nested groups, required session baseline, session.fork marker), the v2 initialize response with required info, and auth/login plus auth/logout. Wire it into run_acp_over alongside v1.


| Field | Value |
|-------|-------|
| Success Criteria | A v2 client negotiates v2; the initialize response carries protocolVersion 2, info, the reorganized capabilities, and authMethods; auth/login and auth/logout respond; a v1 client still gets v1; the v2 acp-tck initialize and auth requirements pass. |
| Complexity | 🔴 High |
| Effort | 6-8h |
| Depends On | T-006 |
| References | acp-v2-server,v2-initialization,v2-authentication,version-negotiation |

### T-008: v2 session lifecycle and config options

Implement session/new, session/list, session/close, session/delete, and session/set_config_option against the same store and ownership leases as v1, with optional mcpServers, cwd, and additionalDirectories, and configOptions carrying a configId and a type discriminator.


| Field | Value |
|-------|-------|
| Success Criteria | The v2 acp-tck session-management, configOptions, and set_config_option requirements pass; sessions created under v2 are listed, closed, and deleted against the same store as v1. |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On | T-007 |
| References | v2-session-lifecycle,v2-config-options |

### T-009: v2 resume replay

Implement session/resume with replayFrom start: replay the full user-visible history as session/update notifications before the response, preserving retained user message ids, then report a current-state snapshot before the response. Reject unsupported cursors with -32602.


| Field | Value |
|-------|-------|
| Success Criteria | The v2 acp-tck resume and replay-ordering requirements pass; a resumed session replays the same user-visible history; a malformed or unsupported cursor is rejected with -32602 rather than silently resumed. |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On | T-008 |
| References | v2-session-replay |

### T-010: v2 foreground state and prompt lifecycle

Implement the v2 session/prompt handler as an insertion-response variant: persist the user message, respond immediately with its messageId, spawn the turn, and drive a per-session foreground state machine that emits state_update transitions (running, requires_action, idle with stop reason). Failures after insertion end as idle(error); pre-insertion rejection stays a JSON-RPC error. Cancellation resolves to idle(cancelled).


| Field | Value |
|-------|-------|
| Success Criteria | The v2 acp-tck prompt-turn, state_update lifecycle, and session/cancel requirements pass; a cancelled turn ends with an idle state_update carrying the cancelled stop reason and no error; no update follows the terminal idle snapshot. |
| Complexity | 🔴 High |
| Effort | 6-8h |
| Depends On | T-008 |
| References | v2-prompt-lifecycle,cancellation-works,protocol-conformance |

### T-011: v2 sink: messages, thoughts, usage

Implement the v2 TurnSink mapping: text_delta to agent_message_chunk, thought_delta to agent_thought_chunk, whole-message upserts with the omit/null/replace patch semantics, usage to usage_update with optional cumulative cost, and session_info to session_info_update. Every message update and chunk carries a required messageId.


| Field | Value |
|-------|-------|
| Success Criteria | The v2 acp-tck keyed-upsert and usage requirements pass; a turn streams chunks and whole-message upserts under one messageId per logical message; usage carries cost only when known. |
| Complexity | 🟡 Medium |
| Effort | 4-5h |
| Depends On | T-010 |
| References | v2-message-updates,v2-usage |

### T-012: v2 tool calls

Report tool calls as tool_call_update upserts keyed by toolCallId, with the first update creating the call, streaming content through tool_call_content_chunk, and terminal status patches replacing accumulated content.


| Field | Value |
|-------|-------|
| Success Criteria | The v2 acp-tck tool-call requirements pass; a model tool call appears as a create-then-patch tool_call_update sequence with no v1 tool_call create notification. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-010 |
| References | v2-tool-calls |

### T-013: v2 permission requests

Implement v2 session/request_permission with a required title and optional subject (tool_call or command), routed through the existing permission pipeline, and report requires_action while foreground work is blocked and running when it resumes.


| Field | Value |
|-------|-------|
| Success Criteria | The v2 acp-tck request_permission requirements pass; a blocked permission reports requires_action and resumes to running; the request carries title and subject; elicitation forwarding never writes the grant store. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-010 |
| References | v2-permission-requests |

### T-014: v2 commands, notices and compaction

Implement available_commands_update with the v2 command input type discriminator, capability-free notice updates, and capability-free compaction updates (compaction_update with compactionId and status, optional compaction_summary_chunk). Reuse the shared dispatch decision for the slash-command ladder.


| Field | Value |
|-------|-------|
| Success Criteria | The v2 acp-tck config/session-info, compaction, and command requirements pass; /context, /mcp, and compaction dispatch identically to v1 through acp::common; notices and compaction updates are sent with no client capability. |
| Complexity | 🟡 Medium |
| Effort | 4-5h |
| Depends On | T-010 |
| References | v2-slash-commands,v2-notices,v2-compaction |

### T-015: v2 MCP integration and elicitation

Connect client-provided mcpServers on session/new and session/resume through the shared MCP pool, advertise session.mcp stdio and http, and serve elicitation/create plus accept elicitation/complete when the client advertises a supported mode.


| Field | Value |
|-------|-------|
| Success Criteria | The v2 acp-tck MCP-config and agent-to-client method requirements pass; a client-provided MCP server is connected and its tools are advertised; elicitation is only sent to a client that advertised a mode. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-008 |
| References | v2-mcp,v2-elicitation |

### T-016: v2 draft session/fork

Implement the draft session/fork surface, advertised under session.fork, creating a seeded fork titled from its parent and reporting it to the client, with the built-in /fork prompt retained as the fallback for clients that do not call it.


| Field | Value |
|-------|-------|
| Success Criteria | A v2 client calling session/fork receives a seeded fork; a v2 client without the capability can still create one through /fork; the v2 acp-tck fork requirement passes or reports SKIPPED, not FAIL. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-008 |
| References | v2-session-fork |

### T-017: v2 conformance burn-down

Run just tackle conformance 2, triage the report, and fix failures until the v2 suite reports CONFORMANT: no MANDATORY failure or untested requirement, no advertised-capability failure, and no version-mismatch or auth block. Burn the v2 baseline down to empty, fixing rather than seeding unless a genuinely inapplicable requirement is documented.


| Field | Value |
|-------|-------|
| Success Criteria | acp-tck reports VERDICT: CONFORMANT for protocol version 2; the committed v2 baseline is empty; the v2 suite exercises negotiation rather than skipping on version mismatch. |
| Complexity | 🔴 High |
| Effort | 8-12h |
| Depends On | T-007,T-008,T-009,T-010,T-011,T-012,T-013,T-014,T-015,T-016 |
| References | v2-conformance-gate,v2-conformance,protocol-conformance |

### T-018: In-process v2 harness and boundary guard

Add a v2 client to the existing in-process ACP harness and tests asserting the v1/v2 boundary: a v1 connection never observes a v2 update and vice versa, and v2 sessions use the same store and leases as v1.


| Field | Value |
|-------|-------|
| Success Criteria | New v2 integration tests drive a turn and a session lifecycle offline; a cross-version leakage test fails if any v2 update is delivered on a v1 connection; the existing v1 suites still pass unchanged. |
| Complexity | 🟡 Medium |
| Effort | 4-6h |
| Depends On | T-007 |
| References | testability,protocol-conformance,v1-preservation |

### T-019: Harness coverage for TCK blind spots

Cover the behaviours acp-tck does not: a v2 client forwarding an MCP server that exposes file or command tools, invoked by the model through the permission pipeline; the draft session/fork and its /fork fallback; compaction and notices gating; and the first-turn absence report when a v2 client brings no MCP server (the client-tooling loss).


| Field | Value |
|-------|-------|
| Success Criteria | Integration tests cover MCP forwarding with a model tool call, session/fork and /fork, capability-free notices and compaction, and the client-tooling-absent path; all run offline. |
| Complexity | 🟡 Medium |
| Effort | 4-6h |
| Depends On | T-009,T-015,T-016,T-018 |
| References | client-tooling-via-mcp,v2-session-fork,v2-notices,v2-compaction,client-tooling-absent |

### T-020: Manual validation against Zed

With both conformance suites green, validate manually against Zed behind its acp-beta flag: negotiate v2, complete a prompt turn with streamed output and a tool call, cancel a turn, resume a session, and observe the client-tooling absence. Record findings, including any client-specific behaviour to document.


| Field | Value |
|-------|-------|
| Success Criteria | Zed negotiates v2 and completes the flows; any divergence from the TCK-verified behaviour is recorded and either fixed or documented; the run is performed only after T-017 passes. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-017,T-018 |
| References | v2-conformance-gate,client-capability-review |

### T-021: Client capability review

Review the ACP v2 client implementations, from primary sources, to establish how each replaces the removed filesystem and terminal surface: whether it forwards an MCP server, ships one, or not at all. The primary question is whether any client exposes editor capabilities as an MCP server. Record findings in research/acp-v2-ecosystem.md.


| Field | Value |
|-------|-------|
| Success Criteria | The ecosystem research records, per reviewed client, its negotiated version and how it exposes file and command tooling under v2, with dated primary-source citations; the open question about MCP-server replacement is answered or explicitly left open with evidence. |
| Complexity | 🟡 Medium |
| Effort | 4-6h |
| Depends On |  |
| References | client-capability-review |

### T-022: Per-client documentation

Add a page per reviewed ACP v2 client under docs/docs/dev/tackle/clients/, recording advertised capabilities, whether it negotiates v2, how it exposes file and command tooling under v2, and any behaviour Tackle must account for. Extend the existing zed.mdx.


| Field | Value |
|-------|-------|
| Success Criteria | Every reviewed client has a page; the pages state the v2 status and the tooling story; the clients index lists them. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-021 |
| References | per-client-documentation |

### T-023: Per-version support matrix

Add a support matrix to docs/docs/dev/tackle stating, per functionality area, whether it is supported under ACP v1 and v2, marking draft-gated functionality, and recording the parity gaps including the client filesystem and terminal tools. Update protocol.mdx to cover both versions.


| Field | Value |
|-------|-------|
| Success Criteria | The matrix names every functionality area with its v1 and v2 support and the draft markers; the client-tooling gap is stated; protocol.mdx describes both versions. |
| Complexity | 🟢 Low |
| Effort | 2-3h |
| Depends On | T-017 |
| References | per-version-support-matrix |

### T-024: Clippy zero

Bring cargo clippy -p agentkit-tackle --all-targets --all-features -- -D warnings to zero. The pre-existing failures are manual_async_fn and await_holding_lock in agent/turn.rs test code, which predate this ADR; fix them and any lints this ADR's work introduces, so the lint gate is green.


| Field | Value |
|-------|-------|
| Success Criteria | cargo clippy -p agentkit-tackle --all-targets --all-features -- -D warnings exits clean, or every remaining lint is a documented false positive with a scoped allow; just lint is green. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-017 |
| References | testability |

