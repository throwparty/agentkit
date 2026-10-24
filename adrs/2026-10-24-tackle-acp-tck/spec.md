# Run acp-tck as a tackle conformance check

**Status:** implemented  **Created:** 2026-10-24  **Author:** adrian

Tackle (adrs/2026-09-17-tackle) is an ACP v1 agent harness. Its NFR-006 requires strict ACP v1 stable compliance "verified by the conformance suite", which today is a single in-repo golden-transcript test (crates/agentkit-tackle/tests/acp_v1_conformance.rs) replaying one recorded lifecycle over raw JSON-RPC. The upstream Agent Client Protocol Test Compatibility Kit (acp-tck, https://github.com/agentclientprotocol/acp-tck) is an external, requirement-level conformance suite: it launches an agent as a stdio subprocess, drives the protocol, and reports each requirement as pass, fail, skipped, or not tested under a CONFORMANT or NOT CONFORMANT verdict. An audit of upstream usage found that acp-tck is run in CI only against deterministic, model-free fixture agents (the Rust SDK's testy and the Python SDK's echo_agent.py), that agent scorecards are diffed against a committed baseline rather than gated, and that no model-backed agent repository gates CI on the TCK; the MCP conformance suite follows the same known-failures-baseline pattern. Tackle needs a model endpoint to answer session/prompt at all, so an offline run needs a deterministic endpoint to exercise prompt and cancellation requirements without network, cost, or flake.


## Problem

Tackle has no external, requirement-level conformance signal. Its only protocol check is an in-repo transcript fixture it authors itself, so drift between tackle and the upstream ACP specification stays invisible until a real client breaks. acp-tck provides that signal but assumes it can launch a working agent, and tackle cannot answer prompt turns offline without a model endpoint.

## Goals

- Run the upstream acp-tck v1 suite against the real tackle binary, offline and deterministically
- Exercise prompt-turn, streaming, and cancellation requirements without a live model, network, or credentials
- Detect new MANDATORY conformance failures continuously while tolerating a committed baseline of known gaps
- Pin acp-tck and its Python environment hermetically so results are reproducible
- Keep the check non-blocking so it informs without gating merges, and keep the existing in-repo transcript suite

## Non-goals

- ACP v2 support, or running the TCK v2 suite
- Making the conformance job blocking, or fixing tackle's current conformance gaps as part of this work
- Validating model output quality or agent behaviour beyond ACP protocol conformance
- Running the TCK over tackle's HTTP transport; the run is stdio only
- Shipping the deterministic test model binary in release artifacts or nix packages
- Replacing or retiring the in-repo golden-transcript suite
- Modifying acp-tck or contributing changes upstream


## Functional Requirements

### FR-001: Conformance Run

A single conformance orchestration recipe runs the pinned upstream acp-tck v1 suite against the release-build tackle binary over stdio, with explicit per-response and per-test timeouts, writing a JSON report to a fixed path and a human-readable verdict summary to stdout.

**Slug:** `conformance-run`

### FR-002: Deterministic Test Model

A separate binary (tackle-test-model, a bin target of the agentkit-tackle crate) serves an OpenAI chat-completions compatible endpoint on localhost, answering streaming SSE responses derived solely from the request content, with no network access and no randomness, so repeated runs produce identical requirement statuses.

**Slug:** `deterministic-test-model`

### FR-003: Cancellation Exercise

The test model recognises the value passed to acp-tck's --cancel-prompt as a sentinel and holds the response stream open for it until the connection is cancelled, so session/cancel requirements are exercised rather than skipped.

**Slug:** `cancellation-exercise`

### FR-004: Tackle Configuration Fixture

A committed configuration fixture points tackle at the test model endpoint through --config-dir and names a model with a known context window so usage reporting is live; the same fixture is used unchanged by local runs and CI.

**Slug:** `tackle-config-fixture`

### FR-005: Runner Owns the Endpoint

The orchestration recipe starts and stops the test model endpoint. Neither acp-tck nor tackle is modified, and the endpoint is never launched by the harness or by tackle itself.

**Slug:** `runner-owns-the-endpoint`

### FR-006: Pinned Toolchain

acp-tck is provided by a pinned nix flake input packaged with uv2nix and exposed in the dev shell; its Python dependencies resolve from its committed uv.lock, and no floating install or network fetch occurs at run time.

**Slug:** `pinned-tck-toolchain`

### FR-007: Baseline Diff

A committed baseline lists the MANDATORY requirement ids expected to fail. The orchestration recipe compares the report against it and exits non-zero only when a MANDATORY requirement fails that is not in the baseline; known failures and a NOT CONFORMANT verdict alone do not fail the run.

**Slug:** `baseline-diff`

### FR-008: Report Artifact

The run always writes the JSON report, including when the baseline diff is non-zero, prints the verdict and per-tier counts, and CI uploads the report as a build artifact.

**Slug:** `report-artifact`

### FR-009: Non-Blocking CI Job

A CI job invokes the orchestration recipe on pull requests touching the tackle crate or the conformance inputs, with continue-on-error so its outcome never gates the pull request or the merge queue.

**Slug:** `non-blocking-ci`

### FR-010: Transcript Suite Retained

The existing in-repo golden-transcript suite stays in cargo test and keeps its role as the fast, focused protocol regression check; acp-tck is additive and does not replace it.

**Slug:** `transcript-suite-retained`

## Non-functional Requirements

### NFR-001: Offline Determinism

The full run needs no network access, no credentials, no model provider, and no external service; identical inputs produce identical requirement statuses across runs.

**Slug:** `offline-determinism`

### NFR-002: Hermetic Pinning

acp-tck, its Python toolchain, and the test model's dependencies are all lock-pinned; updating nix/flake.lock or the pinned uv.lock is the only way the run's inputs can shift.

**Slug:** `hermetic-pinning`

### NFR-003: Bounded Runtime

Every agent interaction is bounded by explicit timeouts and the job completes within the CI budget even when tackle is unresponsive or crashing.

**Slug:** `bounded-runtime`

### NFR-004: Local Parity

Local and CI runs execute the same orchestration recipe and the same committed fixtures, with no CI-only code paths.

**Slug:** `local-parity`

### NFR-005: Non-Interference

The conformance job cannot block a merge, cannot mask a failure in cargo test or clippy, and cannot alter tackle's behaviour.

**Slug:** `non-interference`

### NFR-006: No Product Cost

The test model binary and the conformance inputs add no runtime dependency to tackle and ship in no release or nix artifact.

**Slug:** `no-product-cost`

## Acceptance Criteria

### AC-001: Run Completes

The orchestration recipe exits with the baseline-diff status once the report is written, including when the verdict is NOT CONFORMANT; the report exists at its fixed path and the verdict is printed.

**Slug:** `conformance-run-completes`

### AC-002: Deterministic Across Runs

Two consecutive runs yield identical per-requirement statuses.

**Slug:** `deterministic-across-runs`

### AC-003: Cancellation Exercised

The report shows cancellation requirements as exercised rather than skipped for cancellation not having been exercised.

**Slug:** `cancellation-exercised`

### AC-004: New Failure Detected

Adding a MANDATORY conformance failure absent from the baseline makes the orchestration recipe exit non-zero while still writing the report.

**Slug:** `new-failure-detected`

### AC-005: Baseline Drift Reported

A baseline id that now passes is reported as drift so the baseline can be tightened, without failing the run.

**Slug:** `baseline-drift-reported`

### AC-006: CI Non-Blocking

A red conformance job leaves the pull request's required checks and the merge queue unaffected, and the report artifact is still uploaded.

**Slug:** `ci-non-blocking`

### AC-007: Offline

The run completes with network access disabled.

**Slug:** `offline-run`

### AC-008: Transcript Suite Intact

cargo test still runs and passes the golden-transcript suite independently of acp-tck.

**Slug:** `transcript-suite-intact`

## Edge Cases

### EC-001: Agent Unresponsive

Tackle fails to respond or crashes mid-run; the TCK still completes, every affected MANDATORY requirement is FAIL or NOT_TESTED, the report is written, and the baseline diff reports new failures rather than hanging.

**Slug:** `agent-unresponsive`
### EC-002: Endpoint Port Unavailable

The configured endpoint port is already bound; the orchestration recipe fails fast with a precise error naming the port and never hangs.

**Slug:** `endpoint-port-unavailable`
### EC-003: acp-tck Unavailable

The flake input fails to build or is missing; the orchestration recipe fails with a message that distinguishes a harness problem from a conformance failure.

**Slug:** `tck-unavailable`
### EC-004: Baseline Absent

No baseline file exists; the run fails with an error naming how to seed it from a first report, rather than passing silently.

**Slug:** `baseline-absent`
### EC-005: Requirement Set Changes

A pinned acp-tck update adds or removes requirement ids; newly introduced requirements are evaluated normally, and baseline ids that no longer exist are reported as stale and tolerated until the baseline is refreshed.

**Slug:** `requirement-set-changes`
### EC-006: Cancellation Race

The cancel does not land while the stream is in flight; cancellation requirements report SKIPPED, which the baseline records rather than counting as a conformance regression.

**Slug:** `cancellation-race`

