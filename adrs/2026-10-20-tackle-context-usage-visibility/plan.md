# Tackle Context Usage Visibility Implementation Plan

## Approach

Two tracks land together but remain independently testable. Track A completes the stable ACP usage surface: an optional cost is threaded from the provider response through the turn loop into `UsageUpdate`, and one snapshot helper emits usage after `session/new`, `session/resume`, and `session/load`. Track B adds an ephemeral `/context` command: a built-in discoverability prompt advertises it, the request dispatcher intercepts it before any model request, a report builder reuses the same system prompt, tool catalogue, and prior-message assembly as a real turn, and the command turn is deleted after the reply is streamed.

The first track is a narrow refactor of existing paths (sink signature, turn accounting, lifecycle handlers). The second introduces one new report module and one new handler, and touches dispatch and built-in assets. No new dependency, tokenizer, network call, or persisted schema is introduced.

## Architecture

Cost derivation stays beside the existing context-window lookup in `agent/config_options.rs`: `request_cost` splits an endpoint-qualified model reference, reads the bundled models.dev snapshot's provider entry, and prices provider-reported input and output tokens. It returns `Option<f64>`; `None` means unknowable. The turn loop accumulates known request costs into `TurnUsage.cost_usd`, persists the delta, reads the session's own-turn cumulative usage, and passes `Option<f64>` to the usage callback. `TurnSink::usage` and the ACP sink gain that option and attach `Cost::new(amount, "USD")` only when it is present, preserving the RFD rule that an omitted cost means unchanged.

Lifecycle snapshots move into one helper in `acp_v1/sessions.rs`. The helper resolves the effective model, computes the window, estimates `used` from `assemble_context`, reads cumulative session usage, and sends the update. `session/new` and `session/resume` call it after their responses; `session/load` keeps replay before the response and calls it after the response. Compaction's visible usage drop and the context-overflow fallback share the same update constructor; the overflow fallback sends no cost because it incurred none.

The `/context` report is built in a new `agent/context_report.rs`. It receives the system prompt, the model tool definitions, assembled messages, the current turn id, the window size, and session usage. `agent/prompt.rs` exposes its registry-and-system builder to `pub(crate)` so the report uses exactly the persona, manifest, and tool catalogue a model request would use. Prior messages come from the same assembly path as a real turn; `compaction::prior_messages` is refactored to expose per-message turn kind so compaction summaries can be reported separately without duplicating the filtering and delimiter logic. Text is counted by byte length and images by payload byte length, all through `estimate_tokens`.

The command handler lives in `acp_v1/context.rs` and is wired from `acp_v1/server.rs` before compaction interception. It obtains the session row and actor, builds the runtime and report, streams a generated `agent_message_chunk`, deletes the just-created prompt turn, releases the lease, and responds `end_turn`. The built-in prompt body is only for advertisement and is never expanded. Because the turn is deleted, the command text and report are absent from later assembly and from `session/load` replay, and the session head is restored to its previous turn.

## Technologies

| Technology | Role |
|------------|------|
| Rust | Implementation language for all harness changes |
| agent-client-protocol 2.2 | Stable ACP v1 schema types: `UsageUpdate`, optional `Cost`, `SessionNotification`, and `PromptResponse` |
| agentkit-models | Bundled models.dev snapshot for context windows and per-provider, per-million-token pricing |
| agentkit-tackle store | Existing `SessionStore` assembly, per-turn usage persistence, cumulative session usage, and turn deletion |
| tokio + existing integration harness | Async runtime and the existing in-process ACP test clients used for lifecycle and command tests |

## Components

### pricing

Request cost lookup

Add `request_cost(model_ref, input_tokens, output_tokens) -> Option<f64>` in `agent/config_options.rs`. Split the endpoint-qualified reference, look up `providers[endpoint].models[model]` in `bundled_snapshot_parsed`, and sum known input and output rates. Return `None` when the provider or model is absent or neither rate exists; treat an explicit zero rate as known. No caching layer beyond the existing snapshot call.


### turn-accounting

Turn and session cost accumulation

In `agent/turn.rs`, add a `cost_known` flag alongside `turn_usage`. On each response, add `request_cost` to `TurnUsage.cost_usd` when present, persist the delta, read `session_usage`, and pass `Some(cumulative)` when any cost is known (including a prior turn's known cost) or `None` otherwise. Change the usage callback to `Option<f64>` and keep the request cap, tool-loop, and stop-reason paths unchanged.


### sink

Cost-bearing usage updates

Change `TurnSink::usage` to accept `Option<f64>`. Add a shared `usage_update(used, size, cost)` constructor in `acp_v1/sink.rs` that builds `UsageUpdate::new(used, size)` and attaches `Cost::new(amount, "USD")` only for `Some`. `ConnectionTurnSink`, compaction, lifecycle snapshots, and the overflow fallback all use the constructor so omission semantics are identical.


### lifecycle-snapshots

One snapshot per lifecycle path

Add `send_usage_snapshot` in `acp_v1/sessions.rs`: resolve the model, return early on an unknown window, estimate `used` from assembled content, read cumulative cost, and send exactly one update. Call it after the `session/new` response and command advertisement, after the `session/resume` response and notices, and after the `session/load` response while replay stays before the response. Keep `session/fork` unchanged because the client attaches through `session/load`.


### compaction-usage

Compaction cost continuity

Update the post-compaction `usage_update` in `acp_v1/prompt.rs` to read cumulative session usage and pass it through the shared constructor. The announcement text and RFD-shaped compaction update remain unchanged.


### context-assembly

Shared report inputs

Make `build_registry_and_system` in `agent/prompt.rs` `pub(crate)`. Add `compaction::prior_messages_tagged` (or an equivalent tagged variant) returning each assembled message with its turn kind, and keep `prior_messages` as the thin adapter used by the turn loop. The report uses the tagged form so user, assistant, compaction summary, and image categories are exact and the model path is unchanged.


### context-report

Estimate and render the report

Add `agent/context_report.rs` with a report struct and renderer. Inputs: system prompt, tool definitions, tagged prior messages, window size, and session usage. Count the system prompt and `serde_json` tool catalogue bytes, then per-category text and image payload bytes, convert with `estimate_tokens`, compute percentage and free space only when the window is known, and format aligned rows with thousands separators. Return plain text with an approximate-estimate note and an unknown-window marker when size is zero.


### context-interface

Command semantics and report layout

Interface contract. Trigger: exact first token `/context`, case-sensitive, no arguments; any trailing text is not intercepted and follows normal prompt handling. The command is advertised as `context` with description `Show estimated context-window composition and session usage` and no input hint. Reply: one plain-text `agent_message_chunk` under a generated message id, then `end_turn`; no markdown table, ANSI escapes, emoji, tool-call card, or `usage_update`. Layout is fixed and ASCII-only:

    Context window: ~12,480 / 128,000 tokens (9.8%)
    Model: openai/gpt-4o

      Component                Tokens   Share
      System prompt             1,240    1.0%
      Tool definitions          3,860    3.0%
      User messages               820    0.6%
      Assistant messages        2,140    1.7%
      Compaction summaries      4,420    3.5%
      Images                        0    0.0%
      --------------------------------------
      Used                     12,480    9.8%
      Free                    115,520   90.2%

    Session totals: 48,210 input | 12,340 output | $0.4200
    Estimates use 4 bytes per token; images are counted by payload size.

The table uses a 24-character label column, a 10-character right-aligned token column, and a 7-character right-aligned share column, separated by two spaces. Counts use comma grouping; shares use one decimal place; cost uses four decimal places and USD. Zero categories are always shown. When the window is unknown the header is `Context window: ~<used> tokens (window unknown)`, every share is `-`, and the Free row is omitted. When cost is unknown the footer ends after the output count. When the estimate exceeds the window the percentage is shown as computed and Free is zero. When no model resolves, the handler returns the precise error `no model configured; set [defaults].model or select a model`, after deleting the turn. Tests assert exact golden text for known window and cost, unknown window, unknown cost, and no model.


### context-command

Ephemeral command handler

Add `acp_v1/context.rs` with `run_context_case`. It fetches the session, resolves the actor, builds the runtime, builds the system/tools, assembles context, and renders the report. It streams one `agent_message_chunk` with a generated id, deletes the command turn, releases the lease, and responds `end_turn`. On any error it still deletes the turn and releases the lease before returning a precise JSON-RPC error. No message is appended and no commands are re-advertised.


### command-wiring

Advertisement and dispatch

Add `builtins/context-prompt.md` with description and no parameters, register it as `CONTEXT_PROMPT` in `builtins/assets.rs` and in `defaults()`, and intercept exact `/context` in `acp_v1/server.rs` after actor resolution and before compaction. The registered prompt makes the command appear in `available_commands_update`; the body is never expanded.


## Data Flow

Live turn: provider response -> `request_cost(endpoint/model, input, output)` -> `TurnUsage.cost_usd` delta persisted -> `session_usage` cumulative -> usage callback -> `usage_update` with optional `Cost` -> client.

Lifecycle: `session/new` or `session/resume` response, or `session/load` replay then response -> `send_usage_snapshot` -> resolve model and window -> estimate assembled `used` -> read cumulative cost -> one `UsageUpdate` -> client.

`/context`: prompt request -> lease and prompt turn persisted -> dispatcher matches exact `/context` -> resolve session and actor -> build system prompt and tool catalogue -> assemble prior context -> `context_report` counts bytes and renders text -> stream `agent_message_chunk` -> delete command turn -> release lease -> `end_turn`. The reply never enters storage, next-turn assembly, or load replay.


## Deployment

No deployment change. Cost is computed in-process from the bundled snapshot, snapshots are local notifications, and `/context` makes no provider or network request. Existing stdio and HTTP transports, session database, and multi-process behavior are unaffected; no schema migration or configuration key is added.

