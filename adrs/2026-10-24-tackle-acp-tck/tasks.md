# Run acp-tck as a tackle conformance check — tasks

## Tasks

### T-001: [x] Pinned acp-tck toolchain

Add agentclientprotocol/acp-tck as a nix flake input in nix/flake.nix and package it with uv2nix so its Python environment resolves from the repository's committed uv.lock. Merge the package into the default dev shell and confirm the acp-tck CLI resolves; update nix/flake.lock.


| Field | Value |
|-------|-------|
| Success Criteria | Inside nix develop acp-tck --version prints the pinned version; the flake evaluates and nix flake show succeeds; flake.lock records the acp-tck revision; running acp-tck needs no network access. |
| Complexity | 🔴 High |
| Effort | 3-5h |
| Depends On |  |
| References | pinned-tck-toolchain,hermetic-pinning |

### T-002: [x] Test model bin scaffold

Add src/bin/tackle-test-model.rs to agentkit-tackle with an axum server and clap CLI: --bind (default 127.0.0.1) and --port plus --cancel-sentinel. Register the bin target in Cargo.toml beside tackle-mcp-echo. The process binds localhost only and writes diagnostics to stderr.


| Field | Value |
|-------|-------|
| Success Criteria | cargo build -p agentkit-tackle --bin tackle-test-model succeeds; the binary starts, binds the given address, logs to stderr, and exits cleanly on SIGTERM; release and nix packaging do not include the bin. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On |  |
| References | deterministic-test-model,no-product-cost |

### T-003: [x] Deterministic streaming completions

Implement POST /v1/chat/completions in tackle-test-model: parse the request, derive the reply text and finish_reason solely from the request body, and stream an OpenAI-compatible SSE response with data deltas, a terminal usage chunk, and a non-streaming JSON shape when the request sets stream to false. No randomness, no clocks, no outbound network.


| Field | Value |
|-------|-------|
| Success Criteria | A request with a known body yields byte-identical streamed output across runs; the text and tool-call shapes the tackle provider parses are reproduced; a unit test asserts the SSE framing and the absence of outbound connections. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-002 |
| References | deterministic-test-model,offline-determinism |

### T-004: [x] Cancel sentinel held stream

When a request contains the value passed to --cancel-sentinel, respond with headers and one initial SSE chunk then hold the connection open emitting nothing until the client disconnects or a bounded timeout elapses, keeping a tackle turn in flight so acp-tck cancellation tests exercise session/cancel.


| Field | Value |
|-------|-------|
| Success Criteria | A sentinel request holds the stream open and is observed as not completed; cancelling the client closes the held stream promptly; a non-sentinel request is unaffected. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-003 |
| References | cancellation-exercise,cancellation-exercised |

### T-005: [x] Test model tests

Add offline tests for tackle-test-model: the streaming reply shape for a normal request, the non-streaming reply, the held-open sentinel, and prompt cancellation. Keep them network-free.


| Field | Value |
|-------|-------|
| Success Criteria | cargo test -p agentkit-tackle test_model passes offline; the sentinel test asserts the stream stays open until cancellation; no test touches the network. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-003,T-004 |
| References | deterministic-test-model,cancellation-exercise,offline-determinism |

### T-006: [x] Conformance config fixture

Commit a tackle configuration fixture under crates/agentkit-tackle/tests/conformance/ whose endpoint base_url targets the test model's fixed localhost port with auth none and a model whose context window is known so usage reporting is live; it is passed via --config-dir by the runner.


| Field | Value |
|-------|-------|
| Success Criteria | Pointing the tackle binary at the fixture with --config-dir resolves a model and a context window; a session/prompt against the running test model returns end_turn; the fixture contains no secrets and no paths outside the repo. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-003 |
| References | tackle-config-fixture |

### T-007: [x] Runner recipes and orchestration

Add crates/agentkit-tackle/mod.just with recipes to build the tackle and test model binaries, start the endpoint and record its pid, run the pinned acp-tck with --protocol-version 1 and explicit timeouts and --cancel-prompt set to the sentinel and --report-json to target/ and the tackle command 'agentkit-tackle stdio --config-dir <fixture>' after --, stop the endpoint, and one orchestration recipe that invokes them in order. Reference the crate as a module from the root justfile.


| Field | Value |
|-------|-------|
| Success Criteria | just tackle conformance runs end to end from a clean checkout inside nix develop, offline, and writes the JSON report; the orchestration recipe is the only entry point used by CI; discrete recipes run individually. |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On | T-001,T-005,T-006 |
| References | conformance-run,runner-owns-the-endpoint,local-parity,bounded-runtime |

### T-008: [x] Port handling and cleanup

Make the runner fail fast with a precise error naming the port when the test model address is already bound, and guarantee the endpoint is stopped on every exit path so no orphan process survives a failed or interrupted run.


| Field | Value |
|-------|-------|
| Success Criteria | Starting the orchestration recipe with the port pre-bound fails immediately naming the port and does not hang; after passing, failing, and interrupted runs no test model process remains. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-007 |
| References | endpoint-port-unavailable,bounded-runtime |

### T-009: [x] Baseline checker

Add a stdlib python3 script under crates/agentkit-tackle/tests/conformance/ that reads the acp-tck JSON report, extracts every MANDATORY requirement whose status is not PASS, and diffs that set against the committed baseline. Exit non-zero only when a MANDATORY failure is absent from the baseline; report baseline ids that now pass and ids no longer present as drift; error with seeding guidance when the baseline file is absent. Never delete the report.


| Field | Value |
|-------|-------|
| Success Criteria | Given sample reports the checker exits zero for known failures, non-zero for a new MANDATORY failure, prints drift without failing, and errors clearly when the baseline is missing; it depends only on the python3 standard library. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On |  |
| References | baseline-diff,report-artifact,baseline-absent,requirement-set-changes |

### T-010: [x] Baseline checker tests

Add fixture-driven tests for the checker: known failure tolerated, new MANDATORY failure detected, a NOT CONFORMANT verdict alone not failing, a now-passing baseline id reported as drift, a stale baseline id reported and tolerated, and a missing baseline erroring. Include a report fixture generated by a real acp-tck run.


| Field | Value |
|-------|-------|
| Success Criteria | The tests run offline in CI and cover every branch above; the checker's exit codes are asserted directly. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-009 |
| References | baseline-diff,baseline-drift-reported,new-failure-detected,baseline-absent,requirement-set-changes |

### T-011: [x] Seed the committed baseline

Run the orchestration recipe once against current tackle, review the MANDATORY failures, and commit them as the initial baseline file with a short comment that entries are known gaps to be burned down.


| Field | Value |
|-------|-------|
| Success Criteria | The baseline file exists and is committed; a second orchestration run passes the baseline diff; every baseline id is a real MANDATORY requirement id from the report. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-007,T-009 |
| References | baseline-diff,conformance-run-completes |

### T-012: [x] Non-blocking CI job

Add a reusable workflow (workflow_call) that runs just tackle conformance inside nix develop with continue-on-error, always uploads the JSON report as an artifact, and is invoked from pull-request.yaml on changes touching the tackle crate or the conformance inputs.


| Field | Value |
|-------|-------|
| Success Criteria | The workflow passes actionlint and zizmor; a deliberately failing run still uploads the report and does not fail the pull request's required checks or the merge queue; the job does not run for unrelated changes. |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On | T-011 |
| References | non-blocking-ci,report-artifact,ci-non-blocking,non-interference |

### T-013: [x] Document the conformance posture

Add a short conformance section to docs/docs/dev/tackle/protocol.mdx: what acp-tck covers, that the check is non-blocking and baselined, how to run it locally, and that the in-repo golden-transcript suite remains the fast check.


| Field | Value |
|-------|-------|
| Success Criteria | The docs build succeeds; the page names the orchestration recipe and the baseline; no claim of full conformance is made. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-007 |
| References | transcript-suite-retained,non-blocking-ci |

### T-014: [x] End-to-end verification

Verify the whole feature: run the orchestration recipe with network disabled; run it twice and compare requirement statuses; run cargo test to confirm the golden-transcript suite still passes untouched; and confirm no release or nix artifact includes tackle-test-model.


| Field | Value |
|-------|-------|
| Success Criteria | The run completes with network disabled; two consecutive runs produce identical requirement statuses; cargo test -p agentkit-tackle passes with acp_v1_conformance present; nix and goreleaser outputs contain only the tackle binary. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-012,T-013 |
| References | offline-run,deterministic-across-runs,transcript-suite-intact,no-product-cost |

