# Quest findings — user-facing flows review (2026-09-17)

Review of tackle's user-facing flows against the current ACP v1 schema and the official Rust SDK (`agentclientprotocol/agent-client-protocol`). Findings folded into `spec.toon` requirements.

## Protocol corrections (load-bearing)

1. **HIGH — Unknown update variants do NOT drop harmlessly.** The v1 SDK's `SessionUpdate` is an internally-tagged serde enum with **no catch-all variant**; `compaction_update`/`compaction_summary_chunk` exist only under `unstable_session_compaction` and must only be sent when the client advertised the capability. Sent unconditionally, notifications fail deserialization in stock clients. → All RFD-shaped updates (compaction, notices) capability-gated, advertised via `_meta` in `agentCapabilities`, with stable-variant fallbacks always available.
2. **HIGH — Fork is unreachable/invisible for v1 clients.** Zed will never call `session/fork`; seed chunks on the fork's sessionId go to a session the client never opened. → v1 fallback: built-in user-invokable `/fork [instructions]` that forks, seeds, and reports the new sessionId within the *parent* turn; the fork is openable later via `session/list` → `session/load`. Capable clients: fork updates flow only after the client attaches.
3. **MEDIUM — Message identity.** One `messageId` per logical message; harness-authored messages always get their own ids; same rule in `session/load` replay.

## Flow findings

4. **HIGH — First run with no config.** Ship a built-in default persona/actor so `session/new` always succeeds; missing provider credentials surface via the ACP auth flow; a first-session seed message states what config was loaded/created; config errors name file and key.
5. **MEDIUM — `session/new` latency and silent MCP failures.** MCP connects asynchronously with per-server timeouts; `session/new` returns immediately; per-server status reported at first turn; duplicate server names get precedence + rename rules.
6. **MEDIUM — `session/load` replay semantics.** Full user-visible history in DAG order with original content; compaction boundaries as cards (capable clients) or skipped; ephemeral turns never replayed; exactly one `usage_update` snapshot after replay; stable ids.
7. **MEDIUM — Multi-instance ownership.** Per-session ownership lease with heartbeat, surfaced in `session/list`; concurrent `session/prompt` on an actively-owned session returns a precise error; `session/delete` is soft and never destroys turns referenced by forks; live sessions refuse deletion.
8. **LOW — `session/resume` vs `session/load`.** Prefer `load` for interactive clients; `resume` for headless continuation; distinction documented.
9. **HIGH — `max_turn_requests` ends turns silently.** Emit an agent message before returning it ("reached the model-request limit — say 'continue'"); cap is a visible config option.
10. **MEDIUM — Retries/mid-stream failure indistinguishable from success.** Retry status cards per attempt; final failure message states what happened and what to do; partial output preserved in the DAG.
11. **LOW — Usage cadence.** `usage_update` after each model request within a turn, plus an initial snapshot at session setup.
12. **MEDIUM — `/` vs `!` discoverability and errors.** Prompt frontmatter parameters map to `AvailableCommandInput.hint`; unknown command/arity → precise JSON-RPC error carrying the usage string, nothing stored; `!` errors in-band (failed tool card + agent message with nearest matches); intercept only exact first-token matches so pasted text is never hijacked; both syntaxes documented in the first-session seed and default persona.
13. **MEDIUM — `model_invokable` prompts need a mechanism.** They are exposed to the model as tools whose invocation loads the expanded body into context; the model is instructed never to emit command syntax as text.
14. **LOW — `!` result visibility.** Full result on the tool card, immediate `end_turn`, defined truncation policy for huge outputs.
15. **MEDIUM — Seed comprehension and hook-driven tools.** Seed text is written for the user; harness-authored tool invocations run under a pre-granted permission profile declared in hook config and never surface `session/request_permission`; forks titled at creation.
16. **HIGH — Compaction invisible exactly when it matters.** Always announce in-band with stable variants: agent message ("Compacted: 87k → 12k tokens…") plus `usage_update` so the meter visibly drops; announce at the moment it happens with trigger and counts.
17. **MEDIUM — `/compact` must not appear to no-op.** Defined turn: summarisation → compaction card + confirmation with before/after counts → `end_turn`; on failure, say so, context untouched.
18. **LOW — RFD shape adequacy.** Human-readable counts/trigger inside summary content; machine fields in `_meta`.
19. **HIGH — `allow_always` scope mismatch.** ACP's kind reads as persistent; tackle scopes to the session. → Label honestly ("Allow for this session"); persistent grants are a future decision with a management surface.
20. **MEDIUM — Headless/cancelled permission paths.** Client-cancelled permission requests → reject-once, turn ends `cancelled`; request timeout → reject-once; `reject_always` informs the model once; ask-reasons surface through the permission prompt content.
21. **MEDIUM — Harness work must never prompt.** Harness-authored execution runs no-tools-by-default; hooks declare pre-granted permissions; denials degrade gracefully (skip titling, mark compaction failed).
22. **MEDIUM — Mid-session actor switch.** Validate context fits (auto-compact with announcement if not); inject a context note; grants keyed by actor+tool re-evaluated; full `configOptions` returned; meaningful option descriptions.
23. **LOW — Titling.** Failures silent; `updatedAt` sent each turn; forks titled at creation; never blocks.
24. **MEDIUM — Config adoption transparency.** First-session seed lists loaded layers and counts; built-in inspection prompt; built-in overrides surfaced.
25. **LOW — Compaction disabled + overflow.** Provider context-length errors translated into actionable messages; never retry-looped.
26. **LOW — Conformance coverage.** Mandate a golden-transcript suite: recorded ACP sessions replayed against stable-only features, unstable-feature builds, and a strict deserializer; plus a two-process integration test for session lifecycle.

**Verdict**: the ACP mapping is well-aligned with real v1 surface; correcting the two load-bearing assumptions (capability-gated RFD updates; fork visibility fallback) plus first-run defaults and honest permission scoping makes the design user-facing-ready, with RFD-dependent richness as progressive enhancement.
