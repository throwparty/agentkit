# Tackle Turn Cancellation Implementation Plan

## Approach

One mechanism, landed in dependency order across three tracks. Track A introduces the primitive and its registry: a `tokio_util::sync::CancellationToken` per in-flight turn, created and removed by an RAII registration held in `ConnectionNegotiation` and threaded into the turn runtime. Track B wires the three inbound signals onto that one token, adding no behaviour of their own. Track C finalises and verifies: the ACP stop reason, the baseline reseed, and transcript coverage.

Track A alone is inert until a signal exists, so it ships with a unit test that cancels a token and observes the turn loop stop and persist its partial text. Track B makes the three acp-tck cancellation requirements reachable. Track C closes ACP-CANCEL-001, ACP-CANCEL-002, ACP-CLOSE-002 and ACP-JSONRPC-007 and tightens the committed baseline. No new crate, persisted schema, transport, or client capability is introduced.

## Architecture

A single `CancellationToken` per in-flight turn is the one cancellation primitive. It provides both shapes a turn needs — a `select!`-able `cancelled()` around the in-flight model stream and a synchronous `is_cancelled()` between loop iterations — and it matches the poll-based model the ACP SDK exposes for `RequestCancellation`. Rejected: `futures::future::Abortable`, which is drop-only and one-shot, cannot be checked synchronously or composed into a hierarchy, and cannot be woken by an external signal; and ad-hoc boolean flags, which cannot interrupt an awaiting stream.

The registry is connection-scoped, living in `ConnectionNegotiation` beside the terminal map and keyed by store session id. ACP is point-to-point — one client, one agent, one connection — and the turn task is spawned on the connection, so a process-scoped registry behaves identically for every supported configuration while adding shared mutable state and a deregistration obligation. Rejected: a `TackleState`-wide registry (no behavioural gain) and lease-owner routing (more machinery, no cross-process reach).

All three ACP signals converge on the token. `session/cancel` is a notification carrying a session id; a handler looks up the token and cancels it. `session/close` carries a session id; the close handler cancels the token before running its existing path, so the in-flight prompt resolves cancelled while the close request returns normally. `$/cancel_request` carries a JSON-RPC request id; the SDK records it on the prompt request's `Responder` before handlers run, so the turn captures `responder.cancellation()` and a watcher cancels the same token when it fires. Rejected: a separate code path per signal, which would let them drift.

Cancellation is observed inside the turn loop rather than by dropping the whole turn future. `run_turn` selects the streamed model request against the token so the partially accumulated assistant text stays in scope to persist, and checks the token at the top of each iteration so a cancelled turn issues no further request; tool-call awaits are selected the same way. Rejected: racing the whole `run_prompt_turn` future at its boundary, which drops the loop before the partial text can be persisted and skips the lease release unless duplicated in the wrapper.

`TurnStop` gains a `Cancelled` variant mapped to ACP's `cancelled` stop reason. The response, lease release, registry removal, and the detached post-turn hooks all run on the normal path, so a cancelled turn finalises exactly like a completed one apart from the stop reason.

## Technologies

| Technology | Role |
|------------|------|
| Rust | Implementation language for all harness changes |
| tokio-util CancellationToken | The single cancellation primitive; already present transitively, adopted with its rt feature so no new crate enters the build graph |
| agent-client-protocol 3.5 + schema v1 | `CancelNotification`, `Responder::cancellation`, `StopReason::Cancelled`, and the `on_receive_notification` builder |
| agentkit-tackle turn loop | `agent/turn.rs`, `agent/prompt.rs`, `agent/runtime.rs`, and `acp_v1` handlers; the `ConnectionNegotiation` registry |
| acp-tck and baseline checker | Verification: the four cancellation requirements and the committed baseline diff |

## Components

### cancellation-primitive

Per-turn cancellation token

Add `tokio_util` as a direct dependency with its rt feature. A `CancellationToken` is created per turn and cloned into the turn runtime. Cancellation is signalled once by whichever trigger fires first; the token is idempotent, so repeated or post-completion cancels are no-ops.


### registry

Connection-scoped registration

Add `cancellations: Mutex<BTreeMap<String, CancellationToken>>` to `ConnectionNegotiation`, beside `terminals`, plus `register(session) -> (CancellationToken, CancelRegistration)` and lookup. `CancelRegistration` is an RAII guard that removes the entry on drop, so every exit path (cancel, end_turn, error, connection drop) cleans up. Keyed by store session id.


### turn-runtime

Thread the token into the loop

Add a `cancellation: CancellationToken` field to `TurnRuntime` in `agent/runtime.rs` and set it in `acp_v1/support.rs::turn_runtime`. `run_prompt_turn` passes it to `run_turn_loop`, which passes it to `run_turn`. This keeps the token out of the already-long argument lists.


### turn-observation

Observe cancellation in the loop

In `agent/turn.rs::run_turn`, accumulate streamed text in a shared buffer (the `on_delta` closure appends to it as well as forwarding), then wrap the `provider.stream_completion` await in a biased `tokio::select!` against `token.cancelled()`. On cancel, persist the buffered partial text as an assistant message under `message_id` and return `TurnOutcome { stop: Cancelled }`; the shared buffer avoids a borrow conflict with the still-live stream future. Check `token.is_cancelled()` at the top of each loop iteration and select the tool-call await, so a cancelled turn issues no further request and stops an in-flight tool call. Dropping the stream future drops the reqwest body, closing the connection.


### cancel-notification

session/cancel handler

In the `acp_v1/server.rs` builder, register `on_receive_notification` for `CancelNotification`. The handler resolves the store session id, looks up the token in the connection registry, and cancels it. Unknown sessions are ignored; the notification has no response.


### close-cancel

session/close integration

In `acp_v1/sessions.rs::handle_close_session`, cancel the session's token before the existing close work (store close, grant clear, terminal release). The close request still returns its normal empty response; the in-flight prompt resolves cancelled from its own task.


### cancel-request-bridge

$/cancel_request bridge

In `acp_v1/prompt.rs::run_model_turn_case`, capture `responder.cancellation()` before moving the responder into the spawned task, and have the task cancel the same token when that marker fires. The SDK sets the marker on `$/cancel_request` before handlers run, so this needs no registry.


### stop-reason

Map to ACP stop reason

Add `TurnStop::Cancelled` in `agent/outcome.rs` (re-exported from `agent/mod.rs`) and map it to `StopReason::Cancelled` in `acp_v1/prompt.rs`. `end_turn`, `max_turn_requests`, and `context_length` mappings are unchanged.


### finalization

Graceful turn close

A cancelled turn follows the ordinary tail of `run_prompt_turn`: report session info, fire detached compaction and titling hooks, release the lease, and respond. The registration guard removes the token. No late content or tool-call update is emitted after the response.


### conformance-baseline

Close the gaps and reseed

Extend the deterministic test model flow if needed so `$/cancel_request` is exercised rather than skipped. Run `just tackle conformance`, confirm ACP-CANCEL-001, ACP-CANCEL-002, ACP-CLOSE-002 and ACP-JSONRPC-007 pass, then reseed `crates/agentkit-tackle/tests/conformance/baseline.txt` and confirm no drift.


## Data Flow

Turn start: session/prompt -> dispatch acquires the lease, persists the user turn -> a token is created and registered for the session -> run_prompt_turn is spawned with the token and the responder's cancellation marker -> run_turn streams the model request.

Signal: session/cancel notification, or session/close request, or $/cancel_request -> each resolves the same token -> token cancelled -> the select in run_turn fires -> the streamed request future is dropped, closing the model connection -> partial text is persisted -> TurnOutcome::Cancelled flows up through run_prompt_turn -> session_info, detached hooks, lease release -> registration guard removes the token -> PromptResponse with stopReason cancelled.

Race: a cancel after the response is a no-op because the token is gone and the response already sent; a cancel racing completion resolves through the biased select to exactly one outcome.


## Deployment

No deployment change. Cancellation is entirely in-process; no schema migration, configuration key, transport change, or client capability is added. stdio and HTTP connections behave identically, and a disconnected client needs no cancellation because its spawned turn is dropped with the connection.

