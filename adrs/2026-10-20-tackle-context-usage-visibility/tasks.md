# Tackle Context Usage Visibility Tasks

## Tasks

### T-001: [x] Pricing lookup

Add `request_cost(model_ref, input_tokens, output_tokens) -> Option<f64>` to `agent/config_options.rs`. Split the endpoint-qualified model reference, look up `providers[endpoint].models[model]` in `bundled_snapshot_parsed`, and sum the input and output per-million-token rates. Return `None` when the provider or model is absent or neither rate exists; treat an explicit zero rate as known.


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests cover a priced provider/model, an unknown endpoint, a model missing from a known provider, input-only pricing, output-only pricing, and an explicit zero rate; `cargo test -p agentkit-tackle config_options` passes |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On |  |
| References | cumulative-cost,cost-omission |

### T-002: [x] Optional cost through the sink

Change `TurnSink::usage` to take `Option<f64>`. Add a shared `usage_update(used, size, cost)` constructor in `acp_v1/sink.rs` that attaches `Cost::new(amount, "USD")` only for `Some`. Update `ConnectionTurnSink` and the context-overflow fallback to use it so omitted cost means unchanged.


| Field | Value |
|-------|-------|
| Success Criteria | The crate compiles with the new signature; a sink unit test asserts that `Some` produces a cost-bearing update and `None` omits the field; the overflow fallback still emits a usage update |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-001 |
| References | usage-update-contract,cumulative-cost,cost-omission |

### T-003: [x] Turn and session cost accumulation

In `agent/turn.rs`, track whether any request in the turn was priced. On each response add `request_cost` to `TurnUsage.cost_usd`, persist the delta, read `session_usage`, and pass `Some(cumulative)` when any cost is known, including a prior turn's cost, or `None` otherwise. Keep request-cap, tool-loop, and stop-reason paths unchanged.


| Field | Value |
|-------|-------|
| Success Criteria | A stub-provider turn with a priced model persists the expected per-turn cost and emits a usage update carrying the cumulative session cost; a turn with an unpriced model emits no cost; multi-request turns sum their requests without double counting |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-001,T-002 |
| References | cumulative-cost,cost-omission,usage-update-contract |

### T-004: [x] Lifecycle usage snapshots

Add `send_usage_snapshot` in `acp_v1/sessions.rs`: resolve the effective model, return early when the window is unknown, estimate `used` from assembled content, read cumulative cost, and send exactly one update. Call it after the `session/new` response and command advertisement, after the `session/resume` response and notices, and after the `session/load` response while replay stays before the response. Leave `session/fork` unchanged.


| Field | Value |
|-------|-------|
| Success Criteria | Integration tests count exactly one usage update after new, resume, and load; load replay still precedes the response; each snapshot carries the effective window and cumulative cost when known; unknown-window models emit none |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On | T-002,T-003 |
| References | lifecycle-snapshots,usage-update-contract |

### T-005: [x] Compaction usage cost

Update the post-compaction `usage_update` in `acp_v1/prompt.rs` to read cumulative session usage and pass it through the shared constructor. Leave the announcement text and the RFD-shaped compaction update untouched.


| Field | Value |
|-------|-------|
| Success Criteria | A compaction integration test observes a usage update with the same used/size as today and a cumulative cost when pricing is known; clients without unstable compaction support see no change |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-002,T-003 |
| References | cumulative-cost,cost-omission |

### T-006: [x] Tagged prior messages and shared system builder

Make `build_registry_and_system` in `agent/prompt.rs` `pub(crate)`. Add `compaction::prior_messages_tagged` returning each assembled message with its turn kind, and keep `prior_messages` as a thin adapter for the turn loop. The tagged path must apply the same role filtering, empty-content filtering, and summary wrapping as today.


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests assert that a compaction summary is tagged as a summary and remains wrapped, tool messages are still filtered, and the model turn's prior messages are byte-identical to the current output |
| Complexity | 🟡 Medium |
| Effort | 1-2h |
| Depends On |  |
| References | estimation-basis |

### T-007: [x] Context report builder

Add `agent/context_report.rs` with a report struct and renderer. Inputs: system prompt, tool definitions, tagged prior messages, window size, and session usage. Count system and serialized tool-catalogue bytes, then per-category text and image payload bytes, convert through `estimate_tokens`, compute percentage and free space only when the window is known, and format the fixed ASCII layout with comma grouping and the exact column widths.


| Field | Value |
|-------|-------|
| Success Criteria | Golden-text tests cover a known window and cost, an unknown window with `-` shares and no Free row, unknown cost with the footer ending after output, zero categories, an over-window estimate, and image payload accounting; the renderer uses no ANSI, markdown, or emoji |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-006 |
| References | composition-report,estimation-basis,unknown-window |

### T-008: [x] Context command advertisement

Add `builtins/context-prompt.md` with the description `Show estimated context-window composition and session usage`, no parameters, and a body that is never expanded. Register it as `CONTEXT_PROMPT` in `builtins/assets.rs` and in `defaults()`.


| Field | Value |
|-------|-------|
| Success Criteria | The existing available-commands integration test sees `context` after session/new and session/load; the command has no input hint; the built-in definition loads without a trust prompt |
| Complexity | 🟢 Low |
| Effort | 1h |
| Depends On |  |
| References | context-command |

### T-009: Ephemeral context handler and dispatch

Add `acp_v1/context.rs` with `run_context_case`: fetch the session, resolve the actor, build the runtime, build the system prompt and tools, assemble context, render the report, stream one `agent_message_chunk` with a generated id, delete the command turn, release the lease, and respond `end_turn`. Wire exact `/context` dispatch in `acp_v1/server.rs` after actor resolution and before compaction. On any error delete the turn and release the lease before returning the precise no-model or internal error.


| Field | Value |
|-------|-------|
| Success Criteria | An integration test sends `/context` and receives the report with no model request and no tool card; the prompt turn is deleted and the session head is unchanged; a later turn's assembled context and a session/load replay contain neither the command nor the report; the no-model path returns the specified error after cleanup |
| Complexity | 🔴 High |
| Effort | 3-4h |
| Depends On | T-007,T-008 |
| References | context-command,ephemeral-reply,composition-report |

### T-010: End-to-end usage and context coverage

Extend the existing ACP integration harness with lifecycle and command coverage: cost-bearing updates for a priced endpoint, cost omission for an unpriced endpoint, exactly one snapshot per lifecycle path, unknown-window behaviour, and the ephemeral `/context` round-trip. Reuse the stub provider and in-process client rather than adding a new harness.


| Field | Value |
|-------|-------|
| Success Criteria | The new tests run offline in CI; they assert cost presence and omission, snapshot counts and ordering, unknown-window silence, `/context` output, turn deletion, and no replay; the full `cargo test -p agentkit-tackle` suite passes |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-004,T-005,T-009 |
| References | testability,usage-update-contract,cost-omission,context-report,context-ephemeral |

