# Context Usage Visibility

**Status:** draft  **Created:** 2026-10-20  **Author:** adrian

The ACP session-usage RFD completed on 2026-06-05 and defines stable `UsageUpdate` notifications carrying `used`, `size`, and an optional cumulative session `cost`. Tackle already emits used/size after each model request, once after `session/load`, and after compaction, but never attaches cost, sends no snapshot on `session/new` or `session/resume`, and emits the load snapshot before the load response. Clients that render the update include Zed 1.7.2 (context ring, cost tracked) and acp.nvim (percentage counter); clients that ignore it remain unaffected. Tackle has no `/context` command.

Prior art: Claude Code `/context` shows used/total, percentage, an estimated category breakdown (system prompt, tools, messages, compact buffer, free space), and a grid; Aider `/tokens` shows per-section tokens, cost, and remaining context; Codex exposes `/status` and a context-remaining status line; Gemini CLI exposes `/stats model` and `/stats session`; OpenCode context-indicator plugins show per-category breakdowns.

Tackle assembles model context from a pinned system prompt (persona body plus invokable manifest), the model's tool catalogue, and the turn DAG walked from the head with compaction truncation. Token estimates use a single four-bytes-per-token conversion. The full specification is in adrs/2026-09-17-tackle/spec.toon (FR-007).


## Problem

Users cannot see how a Tackle session's context window is being consumed or what the session has cost. ACP clients that support session usage receive only used and size, and only on some lifecycle paths; cost is always absent. There is no `/context` command, so users cannot identify whether the system prompt, tool schemas, conversation, or images dominate the window, and cannot judge when compaction is worthwhile. The existing local estimate also disagrees with the live client indicator because it ignores the system prompt and tool catalogue.

## Goals

- Complete stable ACP `UsageUpdate` reporting with optional cumulative cost and snapshots on every session lifecycle path
- Add an ephemeral `/context` command that reports approximate context composition and session totals
- Reuse the existing four-bytes-per-token estimate and bundled models.dev metadata without adding dependencies

## Non-goals

- The draft end-turn token usage RFD and its per-turn input/output/thought/cache breakdown
- Exact tokenizer-accurate accounting or per-image token formulas
- Provider-specific cost fields beyond the bundled models.dev pricing snapshot
- Client-side rendering changes or a new protocol extension
- Persisting `/context` output, exposing it to the model, or replaying it on load


## Functional Requirements

### FR-001: Usage Update Contract

Tackle sends a stable `usage_update` after every model request, with `used` equal to that request's provider-reported input tokens and `size` equal to the model's context window from agentkit-models metadata. No update is sent when the window is unknown (size zero). The update is independent of unstable client capabilities; clients that ignore it are unaffected.

**Slug:** `usage-update-contract`

### FR-002: Lifecycle Snapshots

Tackle sends exactly one usage snapshot after `session/new`, after `session/resume`, and after `session/load` replay. Snapshots are sent after the lifecycle response so clients that drop pre-response session updates still receive them. The snapshot derives `used` from the session's assembled context and `size` from the effective model's context window; `session/load` keeps its replay notifications before the response but moves the usage snapshot after it.

**Slug:** `lifecycle-snapshots`

### FR-003: Cumulative Cost

Each `usage_update` carries the session's cumulative cost when it is knowable, using the optional ACP cost field. Cost is computed per request from the bundled models.dev snapshot: the endpoint-qualified model reference is split into endpoint name and model name, the endpoint name is used as the provider key, and input and output token counts are priced at the provider's per-million-token rates. When the provider or model has no pricing, cost is omitted so clients preserve their previous value rather than displaying a misleading zero. Fork sessions price only their own turns.

**Slug:** `cumulative-cost`

### FR-004: Context Command

`/context` is a harness-intercepted command advertised through `available_commands_update` using a built-in discoverability prompt. It is handled before any model request, persists nothing, responds with `end_turn`, and never expands the built-in prompt body into model context.

**Slug:** `context-command`

### FR-005: Composition Report

`/context` returns a plain-text assistant message in the style of Claude Code: a header with estimated used, window size, and percentage; aligned rows for system prompt, tool definitions, user messages, assistant messages, compaction summaries, images, and free space; and a session-totals line with cumulative input tokens, output tokens, and cost when available. Every row is labelled approximate. The report states that the estimate uses four bytes per token.

**Slug:** `composition-report`

### FR-006: Ephemeral Reply

The `/context` reply is streamed as an `agent_message_chunk` under a generated message id and is not stored. The prompt turn created for the command is deleted before the lease is released, so the command text and reply never enter the assembled context, never appear on `session/load` replay, and do not alter the session head.

**Slug:** `ephemeral-reply`

### FR-007: Estimation Basis

The report sums the system prompt and serialized tool catalogue, then the messages the next model request would receive (the same assembly path used for prior messages, current turn excluded), grouped into user, assistant, compaction summaries, and images. Text contributes its byte length; images contribute their payload byte length. All bytes convert through the existing `estimate_tokens` conversion point so `/context`, the overflow fallback, and script context usage agree. No tokenizer is added.

**Slug:** `estimation-basis`

### FR-008: Cost Omission

If pricing is unavailable, the usage update omits cost entirely; it does not send zero and does not invent a currency. Once any request in the session has a known cost, later updates report the cumulative total even if an individual request has no pricing.

**Slug:** `cost-omission`

## Non-functional Requirements

### NFR-001: Consistency

One estimate conversion point and one context assembly path serve `/context`, the context-overflow fallback, and script context usage, so reported estimates cannot drift between surfaces.

**Slug:** `consistency`

### NFR-002: No Dependencies

The feature uses the existing ACP schema, agentkit-models pricing snapshot, and standard library only; no tokenizer, pricing API, or new crate is introduced.

**Slug:** `no-dependencies`

### NFR-003: Testability

Cost arithmetic, omission behaviour, lifecycle snapshot emission, report formatting, and ephemeral turn deletion are covered by unit or integration tests and run offline.

**Slug:** `testability`

## Acceptance Criteria

### AC-001: Cost Reported

With an endpoint name and model present in the bundled pricing snapshot, a completed prompt turn produces a usage update whose cost is the cumulative session cost in USD.

**Slug:** `cost-reported`

### AC-002: Cost Omitted

With an endpoint name absent from the pricing snapshot, usage updates omit the cost field and a client's previously displayed cost is left unchanged.

**Slug:** `cost-omitted`

### AC-003: Snapshots Present

A client receives exactly one usage snapshot after session/new, after session/resume, and after session/load replay, each with the effective model's window and cumulative cost when known.

**Slug:** `snapshots-present`

### AC-004: Context Report

Typing `/context` produces a readable report with used, size, percentage, per-category estimates, free space, and session totals, and triggers no model request.

**Slug:** `context-report`

### AC-005: Context Ephemeral

After `/context`, a subsequent turn's assembled context does not contain the command text or report, and session/load does not replay either.

**Slug:** `context-ephemeral`

### AC-006: Unknown Window

For a model with no context-window metadata, no usage update is emitted and `/context` reports that the window size is unknown rather than dividing by zero.

**Slug:** `unknown-window`

## Edge Cases

### EC-001: Unknown Window

A model without context-window metadata yields size zero: usage updates are skipped, and `/context` still reports the composition estimate with an unknown-window marker and no percentage or free space.

**Slug:** `unknown-window-edge`
### EC-002: Unknown Pricing

A configured endpoint such as a local gateway is not a models.dev provider key. Cost is omitted, while used/size reporting and `/context` continue normally.

**Slug:** `unknown-pricing-edge`
### EC-003: Partial Pricing

A provider prices input but not output, or vice versa. The known side is billed, the request has a known cost, and the cumulative total is reported.

**Slug:** `partial-pricing`
### EC-004: Free Model

A provider explicitly prices a model at zero. Cost is known and reported as zero rather than omitted.

**Slug:** `free-model`
### EC-005: No Model Configured

`/context` fails with a precise error when no model resolves. The turn is still deleted and the lease released; no model request is attempted.

**Slug:** `no-model-configured`
### EC-006: Empty Session

A freshly created session reports the seed turn, system prompt, and tool catalogue with an otherwise empty conversation.

**Slug:** `empty-session`
### EC-007: Images

Images are counted by payload bytes and shown as their own category. The report labels this estimate approximate because image tokenisation is model-specific.

**Slug:** `images`
### EC-008: Concurrent Prompt

`/context` follows the existing ownership lease rules: a prompt against a session owned by another connection returns the same precise ownership error as any other prompt.

**Slug:** `concurrent-prompt-edge`

