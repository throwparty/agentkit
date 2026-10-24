# Tackle Turn Cancellation

**Status:** implemented  **Created:** 2026-10-24  **Author:** adrian

Tackle (adrs/2026-09-17-tackle) is an ACP v1 agent harness. ACP v1 defines three inbound signals that stop an in-flight session/prompt: the session/cancel notification, the session/close request, and the protocol-level $/cancel_request notification, which the SDK records on the request's Responder before handlers run. The umbrella tackle specification already requires the behaviour — FR-003 states that session/close cancels ongoing work and releases the ownership lease — and docs/docs/dev/tackle/acp.mdx names cancelled among the turn's terminal stop reasons, but no implementation mechanism is specified.

The implementation today has none of it. There is no session/cancel handler; a prompt turn is spawned detached (so the dispatch loop stays free to route permission replies) and runs to completion; session/close closes the store row, drops grants and releases terminals without touching the running turn; and $/cancel_request is ignored. The pinned acp-tck suite records these gaps in crates/agentkit-tackle/tests/conformance/baseline.txt: ACP-CANCEL-001, ACP-CANCEL-002 and ACP-JSONRPC-007 (MANDATORY), with ACP-CLOSE-002 (CAPABILITY) failing too. A followed-on advisory gap, ACP-INIT-004 (agentInfo), was fixed separately and is not part of this ADR.

The turn's one long await is a single streamed model request (crates/agentkit-tackle/src/agent/turn.rs), so a mechanism must drop that stream promptly and then finalise the turn: persist partial output, release the lease, and answer with stopReason cancelled. The ACP SDK (agent-client-protocol 3.5.0) models cancellation cooperatively, exposing RequestCancellation with cancelled(), is_cancelled() and run_until_cancelled(); session/cancel and session/close carry a session id rather than a request id, so tackle needs its own session-to-turn index alongside the SDK's per-request marker.


## Problem

An in-flight session/prompt cannot be cancelled. A client that sends session/cancel gets no response until the model stream ends on its own; session/close returns before the running turn stops; and $/cancel_request has no effect. Four acp-tck requirements fail, three of them MANDATORY, and a turn can hold its session lease and its model connection indefinitely regardless of client intent.

## Goals

- Cancel an in-flight prompt turn from any of the three ACP signals, answering with stopReason cancelled
- Route all three signals through one cancellation mechanism so their behaviour cannot drift
- Finalise a cancelled turn cleanly: persist partial output, release the lease, and emit no updates after the response
- Close the acp-tck MANDATORY gaps and tighten the committed baseline

## Non-goals

- Persisting or reconciling cancellation across process restarts; a restarted process has no in-flight turns
- Cancelling a turn from a different connection or process than the one running it
- A general job-control or pause/resume facility beyond stopping an in-flight turn
- Client-side changes or a new protocol extension; this uses existing ACP v1 signals only
- Fixing unrelated conformance gaps, including the advisory agentInfo requirement already addressed


## Functional Requirements

### FR-001: Session Cancel

A session/cancel notification addressed to a session with an in-flight prompt turn cancels that turn: it stops streaming, finalises, and the session/prompt resolves with stopReason cancelled. A session/cancel for a session with no in-flight turn is ignored, as the notification has no response.

**Slug:** `session-cancel`

### FR-002: Close Cancels

A session/close request for a session with an in-flight prompt turn cancels that turn as if session/cancel had been sent, per the umbrella session-lifecycle requirement. The prompt resolves with stopReason cancelled and the close request still returns its normal response; the existing close behaviour (the session stays listable, grants are dropped, harness-created terminals are released) is preserved.

**Slug:** `close-cancels`

### FR-003: Cancel Request

A $/cancel_request targeting an in-flight session/prompt produces exactly one response to that request: a schema-valid PromptResponse with stopReason cancelled, or an error with code -32800. No second response follows.

**Slug:** `cancel-request`

### FR-004: Partial Output Persisted

Assistant text already streamed before cancellation is persisted to the turn; a turn that produced no model output persists nothing extra. Cancellation ends the turn without error noise.

**Slug:** `partial-output-persisted`

### FR-005: One Mechanism

session/cancel, session/close and $/cancel_request all signal the same per-turn cancellation state, so a turn cannot be cancelled by one path and left running by another.

**Slug:** `one-mechanism`

### FR-006: No Updates After Response

After the cancelled session/prompt response, no further content or tool-call session/update for that session is emitted. Session-state updates allowed outside a turn (for example session_info_update and usage_update) are unaffected.

**Slug:** `no-updates-after-response`

### FR-007: Cleanup On Cancel

Cancelling a turn releases its ownership lease and removes its cancellation registration, leaving the session available for a subsequent prompt.

**Slug:** `cleanup-on-cancel`

### FR-008: Idempotent Cancellation

Cancellation is idempotent and race-safe: a signal that arrives after the turn has already resolved is a no-op, and a cancellation racing turn completion yields exactly one response.

**Slug:** `idempotent-cancellation`

## Non-functional Requirements

### NFR-001: Single Primitive

One cancellation primitive (a tokio-util CancellationToken) carries every signal, matching the poll-based shape the ACP SDK already exposes for RequestCancellation; no parallel ad-hoc flags.

**Slug:** `single-primitive`

### NFR-002: No New Dependency Tree

tokio-util is already present transitively, so adopting it enables a feature rather than adding a crate to the build graph. No other new dependency is introduced.

**Slug:** `no-new-dependency-tree`

### NFR-003: Cooperative And Non-Blocking

Cancellation is cooperative: the dispatch loop never blocks on a turn, and signalling cancellation is cheap and cannot deadlock. A provider that ignores the dropped stream cannot block the connection.

**Slug:** `cooperative-and-non-blocking`

### NFR-004: Offline Testability

Cancellation paths are covered offline against the deterministic test model and by the in-repo golden-transcript suite.

**Slug:** `offline-testability`

## Acceptance Criteria

### AC-001: Cancel Conformance

acp-tck reports ACP-CANCEL-001 and ACP-CANCEL-002 as PASS.

**Slug:** `cancel-conformance`

### AC-002: Close Conformance

acp-tck reports ACP-CLOSE-002 as PASS.

**Slug:** `close-conformance`

### AC-003: Cancel Request Conformance

acp-tck reports ACP-JSONRPC-007 as PASS and exercised rather than skipped.

**Slug:** `cancel-request-conformance`

### AC-004: Baseline Tightened

crates/agentkit-tackle/tests/conformance/baseline.txt no longer lists the cancelled requirement ids, and the runner reports no drift.

**Slug:** `baseline-tightened`

### AC-005: Transcript Coverage

The golden-transcript suite covers a cancelled turn and a close during a turn, and fails if a late update follows the response.

**Slug:** `transcript-coverage`

### AC-006: Graceful Idle Close

Closing or cancelling a session with no in-flight turn behaves exactly as today.

**Slug:** `graceful-idle-close`

## Edge Cases

### EC-001: Cancel After Completion

A session/cancel that arrives after the prompt response has been sent is a no-op: no second response and no error.

**Slug:** `cancel-after-completion`
### EC-002: Unknown Session

session/cancel for an unknown or closed session is ignored; session/close for an unknown session keeps the existing response behaviour.

**Slug:** `unknown-session`
### EC-003: Idle Close

session/close with no in-flight turn performs the existing close path unchanged.

**Slug:** `idle-close`
### EC-004: Cancel During Tool Call

Cancellation while a tool call is in flight stops the turn; an open permission request resolves reject-once per the existing permission pipeline, and the session/request_permission outcome is unchanged.

**Slug:** `cancel-during-tool-call`
### EC-005: Connection Drops

A connection dropping mid-turn drops the spawned turn task with it; there is no orphaned task and no cancellation signal is required.

**Slug:** `connection-drops`
### EC-006: Race With Completion

A cancellation racing the model's natural completion yields exactly one response, cancelled or end_turn, never both.

**Slug:** `race-with-completion`
### EC-007: Nested Agent Loop

A cancelled turn also stops a nested agent.<actor> loop it spawned, because the nested loop runs inside the turn's own future and is dropped with it.

**Slug:** `nested-agent-loop`

