# Tackle Implementation Tasks

## Tasks

### T-001: [x] Crate scaffolding and CLI

Create crates/agentkit-tackle as a workspace member with the twelve-module tree; implement cli.rs with clap: --transport stdio|http (default stdio), --bind, --http-port (default 127.0.0.1:3811), --db-path, --config-dir, --version; graceful shutdown on SIGINT/SIGTERM; all diagnostics to stderr


| Field | Value |
|-------|-------|
| Success Criteria | cargo run -- --help shows all flags; SIGTERM exits cleanly without corrupting stdout; a stub server accepts a connection |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On |  |
| References | acp-v1-server |

### T-002: [x] agentkit-path config_dir

Add config_dir alongside the existing data_dir in agentkit-path: platform config directory per the CONTEXT.md convention (~/.config/agentkit/<component> on Linux)


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests: config_dir returns the platform path and joins component names; existing data_dir tests still pass |
| Complexity | 🟢 Low |
| Effort | 1h |
| Depends On |  |
| References | config-discovery |

### T-003: [x] Layered configuration

Implement config loading: user layer then project ./.agentkit/tackle/config.toml with project precedence; reject endpoint and credential fields in project configuration with named errors; project mcp_servers entries are accepted but trust-gated (T-005); config.example.toml is the schema of record


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests: merge precedence, rejection of [endpoints] in project config with the offending key named, valid layered load |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On | T-002 |
| References | config-discovery,model-endpoint-config |

### T-004: [x] Definition loading

Discover and parse personas, actors, prompts, and scripts from config subdirectories; frontmatter dispatch: --- as YAML via yaml_serde, +++ as TOML via toml; bounded parsing with size caps; validation errors name file and key; built-in default persona and actor compiled in via include_str as fallbacks


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests: both frontmatter formats parse identically, malformed definitions error with file and key named, empty config yields the built-in defaults |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-003 |
| References | first-run,config-discovery,reusable-prompts |

### T-005: [x] TOFU trust records

Maintain trust.toml in the user config directory: normalized repository remote mapping relative paths to SHA-256 hashes for project scripts, personas, actors, prompts, and mcp_servers entries; first-use consent flow behind a stubbed prompt interface, replaced by request_permission wiring in T-022; re-consent on hash change; load once at start, never hot-reload


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests: trust-record lifecycle, hash-change detection, untrusted project definitions and servers are refused before consent |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On | T-003,T-004 |
| References | config-discovery,security-posture |

### T-006: [x] Storage module core

Implement the storage module adopting the schema.sql DDL: sessions, turns, messages, definitions, turn_definitions tables; SessionStore trait with SQLite (sqlx, WAL, busy_timeout, 0600 file perms) and in-memory backends; sqlx migrations; per-turn usage deltas on turns; permission grants are in-memory only and deliberately absent from the schema


| Field | Value |
|-------|-------|
| Success Criteria | Migration tests against in-memory SQLite; CRUD round-trips on both backends; file permissions verified |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On | T-001 |
| References | session-storage,session-lifecycle |

### T-007: [x] Context assembly and daggy mirror

Implement the recursive CTE walk from the session head stopping at compaction turns with first_retained_turn_id resume; the in-memory daggy::Dag mirror maintained DB-first and rebuilt on conflict; property tests asserting SQL walks match daggy traversals


| Field | Value |
|-------|-------|
| Success Criteria | Property tests: SQL and daggy orderings identical across generated DAG shapes; assembly cases: fork before and after compaction, multiple compactions, reversibility |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On | T-006 |
| References | session-storage,compaction,testability |

### T-008: [x] Ownership leases and soft delete

Implement per-session ownership lease with heartbeat and expiry; session/prompt against an actively-owned session fails with a precise error; soft delete sets active=0, preserves turns referenced by forks, refuses live sessions


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests: lease acquisition and contention, heartbeat refresh, soft-delete invariants with fork-referenced turns |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-006 |
| References | session-ownership,multi-instance |

### T-009: [x] ACP server core

Implement the Agent trait over AgentSideConnection with the stdio transport; initialize handling with protocol version negotiation responding with the latest supported version


| Field | Value |
|-------|-------|
| Success Criteria | Integration test: initialize handshake with a recorded client transcript; version negotiation responds correctly to v1 requests |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On | T-001 |
| References | acp-v1-server |

### T-010: [x] Capability advertisement

Advertise loadSession, sessionCapabilities for list close resume delete, promptCapabilities image, mcpCapabilities http; unstable features (compaction updates, notices, fork) behind SDK cargo features and _meta gates, sent only when the client advertises support


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests: capability map contents; gating tests: unstable updates withheld from clients without advertised support |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-009 |
| References | capability-advertisement,protocol-conformance |

### T-011: [x] Session lifecycle methods

Implement session/new (immediate return, default actor selection, MCP connect kicked off asynchronously), session/list (updatedAt descending, cwd filter, cursor pagination, include_ephemeral option with _meta marking, ownership lease surfaced per session), session/close (cancels work, releases the lease, session stays listable), session/delete (soft-hides from list)


| Field | Value |
|-------|-------|
| Success Criteria | Integration tests: lifecycle round-trip on recorded transcripts; list ordering, filtering, ephemeral marking, and lease surfacing; delete hides, close does not |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-009,T-006,T-008 |
| References | session-lifecycle,session-ownership,first-run,ephemeral-sessions |

### T-012: [x] Session load and resume

Implement session/load replaying full user-visible history in DAG order with original content, stable message and tool-call ids, turns of ephemeral sessions excluded, exactly one usage snapshot after replay; session/resume reattaching without replay


| Field | Value |
|-------|-------|
| Success Criteria | Integration tests: replay matches recorded transcripts with stable ids; resume returns without notifications; ephemeral-session turns never replayed |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-011,T-007 |
| References | session-lifecycle,ephemeral-sessions |

### T-013: [x] Golden-transcript conformance harness

Build the fixture infrastructure: recorded ACP sessions as golden transcripts, replayed against stable-only features, unstable-feature builds, and a strict deserializer; wired to run in CI


| Field | Value |
|-------|-------|
| Success Criteria | The harness runs recorded transcripts against all three build configurations and reports diffs; CI job green |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On | T-012 |
| References | testability,protocol-conformance,capability-advertisement |

### T-014: [x] ModelProvider trait and rig-core implementation

Implement the tackle-owned ModelProvider trait; rig-core implementation speaking per-endpoint wire format (openai-chat-completions and anthropic-messages); named endpoints with endpoint-qualified models; credential resolution through the credential helper command with the endpoint name as identity


| Field | Value |
|-------|-------|
| Success Criteria | Cassette-based tests: completion round-trips per wire format; credential helper invoked with the correct identity; no secrets in logs |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On | T-003 |
| References | model-endpoint-config |

### T-015: [x] Turn loop

Implement the turn loop: context assembly via the store, system prompt builder (persona body plus manifest sections), streaming with chunk accumulation and one messageId per logical message, stop-reason mapping onto ACP values, configurable model-request cap (default 8) with an explanatory agent message before max_turn_requests; buildable against stub registry and pipeline — integration with the real ones lands in T-021 and T-022


| Field | Value |
|-------|-------|
| Success Criteria | Cassette-based tests: multi-request turns with tool calls against stubs; messageId discipline; cap announcement; content-type validation rejecting unsupported blocks |
| Complexity | 🔴 High |
| Effort | 6-8h |
| Depends On | T-014,T-007,T-004 |
| References | turn-loop,manifest-injection,content-types |

### T-016: [x] Usage reporting

Emit usage_update after each model request and once at session setup; used from the last request input tokens, size from agentkit-models context-window metadata, cost from agentkit-models pricing or endpoint-reported values when present; per-turn usage deltas persisted on turns, session cumulative cost summed over own turns so forks never double-count


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests: usage values derived correctly; snapshot at setup; updates after each request; per-turn persistence and fork attribution |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-015 |
| References | usage-reporting |

### T-017: [x] Error handling

Implement retries with exponential backoff honouring retry-after, reported as status cards; hard failures as JSON-RPC errors; mid-stream failures as agent message plus terminal stop reason; partial output preserved in the DAG; provider context-length errors translated into actionable messages without retry loops


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests: retry schedules, retry-after honouring, mid-stream failure shape, context-length translation, no retry loops |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-015 |
| References | error-handling |

### T-018: [x] MCP client pool

Implement the rmcp client pool: connections from client-provided mcpServers, config mcp_servers (transport stdio|http), and trust-gated project entries; asynchronous connect with per-server timeouts so session/new never blocks; per-server status surfaced at the first turn; duplicate name precedence with client winning and shadowed servers reported; tool results truncated at 16 KiB with a size marker


| Field | Value |
|-------|-------|
| Success Criteria | Integration tests with mock stdio and HTTP servers: connect, hang, failure, duplicate precedence, first-turn status, truncation |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On | T-003,T-005,T-011 |
| References | mcp-integration |

### T-019: [x] Spawn hygiene and stderr relay

Spawn stdio servers with explicit argv, no shell interpretation, named env entries only, no blanket environment forwarding; relay child stderr to tackle's stderr line-prefixed with the server name and rate-capped, never touching stdout


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests: spawn argv construction, env filtering, stderr relay prefixing and rate cap, stdout purity |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-018 |
| References | mcp-integration,observability |

### T-020: Elicitation forwarding

Forward MCP elicitations to session/request_permission with explicit origin attribution semantically distinct from tool permission prompts; elicitation responses never write the grant store


| Field | Value |
|-------|-------|
| Success Criteria | Integration tests: elicitation surfaced with origin, responses never create grant records |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-018,T-022 |
| References | mcp-integration,permission-pipeline |

### T-021: [x] Invokable registry

Implement the unified registry: mcp., prompt., and agent. namespaces with user_invokable and model_invokable flags; collisions are configuration errors at load; available_commands_update with parameter hints mapped from prompt frontmatter; the registry is the only dispatch point — the harness ships no built-in tools beyond it


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests: registration, namespacing, collision errors, command advertisement with hints |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-004,T-018 |
| References | invokable-registry,reusable-prompts,thin-core |

### T-022: [x] Permission pipeline

Implement the fixed-order pipeline: grant deny records authoritative, then the pre_tool_use policy script (allow and deny final, ask falls through, previously_granted flag), then allow_always honouring, then the fixed ask fallback regardless of the interactive indicator; the grant store is in-memory only — keyed by actor and invokable, written by tool-invocation allow_always and reject_always, gone on close, process exit, and resume after restart; request_permission integration with honest session-scope labels, cancelled and timed-out requests as reject-once, ask reasons in the prompt content; wires the TOFU consent flow from T-005 to request_permission


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests: pipeline order, veto layer, fail-closed, headless auto-reject, timeout, grant lifetime (close, exit, resume) and actor keying |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On | T-006,T-009,T-005 |
| References | permission-pipeline,security-posture |

### T-023: Prompt expansion

Implement /name expansion with parameter substitution into the conversation for the model; model_invokable prompts exposed as tools whose invocation loads the expanded body; unknown commands and arity mismatches fail with a precise JSON-RPC error carrying the usage string, storing nothing


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests: expansion with parameters, model-tool invocation loading the body, error paths with usage strings and no stored state |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-021 |
| References | reusable-prompts,invokable-registry,manifest-injection |

### T-024: Direct invocation

Implement /!name execution of MCP tools with no model request: a command named ! is advertised via available_commands_update so clients autocomplete the prefix; execution reports tool_call and tool_call_update, responds end_turn, stores the result in the turn, and bypasses the pipeline; exact first-token match on user-typed input only; oversized outputs truncated at 16 KiB with an explicit size marker, full content retained in storage


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests: execution shape, bypass, advertised ! command, interception rejecting pasted text and model output, truncation marker |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-021 |
| References | direct-invocation |

### T-025: Rhai engine builders

Implement the two structurally separated engine builders: policy engines with decision inputs only (request, actor info, interactive indicator, history_search) physically lacking the acting functions; behaviour engines with the full host API; limits: operations, call levels, expression depths, collection sizes, 256 KiB script size, ~1s policy and ~10s behaviour time via on_progress, on_parse_token parse protection, no modules, fresh engine per invocation, argument payload caps; fail-closed on error or timeout; on_print/on_debug to stderr; catch_unwind on host functions


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests: limit enforcement, fail-closed paths, stdout hygiene, panic safety, engine separation (policy engines cannot act) |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On | T-001 |
| References | policy-scripts,security-posture |

### T-026: Behaviour host API

Implement SessionAccess and the behaviour host functions: fork_session (ephemeral option, usage attribution to parent), send_prompt, await_completion (timeout), insert_seed, record_compaction, set_session_title, history_search, context_usage, log; fixed internal budgets: live ephemeral forks, prompts per script and turn and session, cost cap at the parent, depth-one nesting rule; script-driven model calls surfaced in the client


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests: each host function against the in-memory store; budget exhaustion; depth-one enforcement; attribution |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On | T-025,T-007 |
| References | behaviour-scripts,ephemeral-sessions,thin-core |

### T-027: Forking

Implement session/fork for clients advertising support with updates flowing only after attach; the /fork fallback prompt for v1 clients reporting the new session id in the parent turn; session_forked script event; seed messages as harness-authored static user-facing content with their own messageId; forks titled from their parent at creation


| Field | Value |
|-------|-------|
| Success Criteria | Integration tests: fork via RFD path and fallback path; seed insertion; fork titles; script failure never fails the fork |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-011,T-023,T-026 |
| References | forking,ephemeral-sessions |

### T-028: Compaction

Implement compaction turns with the in-band announcement (trigger and token counts plus a visible usage_update drop); the /compact turn definition: compaction-tagged prompts are intercepted, the compaction_requested event fires instead of a model turn, and the harness announces with before and after counts; immutable pinned prefix that compaction can never elide; records attributed to the producing script; compaction-disabled context-length errors translated into actionable messages naming the script


| Field | Value |
|-------|-------|
| Success Criteria | Integration tests: compaction reduces assembled context, announcement contents, pinned prefix survives, keep-recent resume, reversibility by deletion, disabled-overflow message |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On | T-007,T-023,T-026 |
| References | compaction,error-handling |

### T-029: Session titling

Implement the default title_trigger script: ephemeral-fork summarisation into a six-word title, set via session_info_update; failures silent and never blocking; updatedAt sent each turn


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests: titling flow, silent failure, updatedAt cadence |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-026 |
| References | session-titling |

### T-030: First-run experience

Implement session/new succeeding with empty configuration via built-in defaults; ship the built-in asset inventory: default persona, default actor, summariser actor, compaction and titling scripts, /fork and /compact prompts; first-session seed message listing loaded configuration layers with counts and documenting the / and /! syntaxes; configuration errors naming file and key; built-in override surfacing; missing provider credentials surface via authMethods advertisement and the ACP authenticate method


| Field | Value |
|-------|-------|
| Success Criteria | Integration tests: first run with empty config, seed contents, error messages, override surfacing, auth flow on missing credentials |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-011,T-004,T-022 |
| References | first-run,config-discovery |

### T-031: Session config options

Implement FR-027: configOptions with a model selector (per-endpoint discovery via rig's model-listing support, static fallbacks, degraded-discovery notice) and an actor selector; mid-session model switch validated against the new model's context window with auto-compaction announcement and a context note, effective the following turn


| Field | Value |
|-------|-------|
| Success Criteria | Integration tests: selector population with discovery and fallback, switch semantics, window validation, config_option_update |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-011,T-014,T-016 |
| References | session-config-options |

### T-032: Observability

Emit OTel spans per the switchboard-otel pattern for turns, tool calls, and token usage; credentials redacted at the type level via secrecy; tool arguments redacted in logs; all diagnostics to stderr


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests: span presence and hierarchy, redaction assertions, stderr discipline |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-015,T-018 |
| References | observability,security-posture |

### T-033: Two-process integration test

Integration test with two tackle processes sharing one session database: session/list visibility across processes, lease contention producing precise errors, fork lineage across processes


| Field | Value |
|-------|-------|
| Success Criteria | The two-process test runs in CI; contention and visibility assertions pass |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On | T-012,T-008 |
| References | multi-instance,session-ownership,testability |

### T-034: Conformance and release polish

Complete the golden-transcript suite coverage across all flows (prompt turns, permissions, forks, compaction, direct invocation); full suite green offline; binary release via the workspace release process and nix


| Field | Value |
|-------|-------|
| Success Criteria | CI green: conformance suite across all three build configurations, two-process test, full offline suite; release artifact builds |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On | T-013,T-032,T-033 |
| References | testability,protocol-conformance,performance |

### T-035: Agent invocation

Implement FR-026: agent.<actor> invokables spawn a new agent instance of that actor running a nested turn loop within the current turn; the nested loop shares the parent turn's model-request cap; one-level nesting limit (a sub-agent cannot spawn further agents); usage attributed to the parent session; model-invokable only


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests: nested loop round-trip, cap sharing, nesting limit refusal, usage attribution |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-015,T-021,T-022 |
| References | agent-invocation |

### T-036: ACP HTTP transport

Implement the HTTP transport via agent-client-protocol-http over axum: --bind and --http-port wiring, multiple client connections in one process, integration test driving a session over HTTP


| Field | Value |
|-------|-------|
| Success Criteria | Integration test: initialize through session lifecycle over HTTP; multiple concurrent connections served |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-009 |
| References | acp-v1-server |

