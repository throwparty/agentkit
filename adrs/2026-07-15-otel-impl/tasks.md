---

## status: draft created: 2026-07-15 updated: 2026-07-16 author: adrian decision: pending

# Tasks: OpenTelemetry Implementation for Rust Services

## Task List

| #   | Task                                             | Est.  | Depends On     | Spec                      | Plan                         |
| --- | ------------------------------------------------ | ----- | -------------- | ------------------------- | ---------------------------- |
| 1   | Scaffold PoC crate and OTel SDK wiring           | 1 day | —              | §2.1, FR4, FR6, NFR1      | §3.1, §3.2, §5.2             |
| 2   | Build tracing-to-OTel log layer                  | 1 day | Task 1         | FR3, FR5                  | §3.3.3                       |
| 3   | Wire traces and metrics, compose subscriber      | 1 day | Task 1, Task 2 | FR1, FR2, FR5, NFR5, NFR1 | §3.3.1, §3.3.2, §3.3.4, §3.5 |
| 4   | Write offline in-memory unit tests               | 1 day | Task 3         | FR1–FR6, NFR4             | §4.1                         |
| 5   | Manual integration test with otel-desktop-viewer | 1 day | Task 4         | All FRs, NFR1–NFR5        | §4.2, §4.4, §4.5             |

---

> **Note**: The original Task 6 ("Integrate OTel into agentkit-switchboard") was moved to
> `adrs/2026-09-06-switchboard-otel`. This ADR covers the PoC only.

---

### Task 1: Scaffold PoC crate and OTel SDK wiring

**Summary**: Create the `poc-otel/` crate with `Cargo.toml` and `setup.rs`. The setup module initialises the OTel Resource, OTLP HTTP/protobuf exporter, and all three providers (TracerProvider, MeterProvider, LoggerProvider) with batch processing. Returns a `ShutdownGuard` that flushes all providers on drop.

**Depends on**: Nothing.

**Acceptance Criteria**:

- `adrs/2026-07-15-otel-impl/poc-otel/Cargo.toml` exists with pinned versions matching plan §2.5
- `cargo build` succeeds in the crate directory
- `setup.rs` exports `init_otel()` returning `(TracerProvider, MeterProvider, LoggerProvider, ShutdownGuard)`
- Resource reads `OTEL_SERVICE_NAME` env var with fallback `"poc-otel"`
- OTLP exporter reads `OTEL_EXPORTER_OTLP_ENDPOINT` with fallback `http://localhost:4318`
- `ShutdownGuard::drop()` calls `shutdown()` on all three providers without panicking
- Providers are wired with OTLP exporter explicitly (not relying on auto-configuration env vars like `OTEL_TRACES_EXPORTER`)

**Test expectations**: Not yet — Task 4 covers all tests. Verify by compiling.

**Files to create**:

- `poc-otel/Cargo.toml` — dependencies from plan §3.1.1, pin exact versions
- `poc-otel/src/setup.rs` — `init_otel()`, `ShutdownGuard`
- `poc-otel/src/main.rs` — minimal entry that calls `init_otel()` and exits (placeholder for Task 3)

**Rollout**: Throwaway crate under `adrs/`. No risk to workspace crates.

---

### Task 2: Build tracing-to-OTel log layer

**Summary**: Implement a `tracing_subscriber::Layer` that intercepts `tracing::Event` records and forwards them to the OTel `Logger`. This is the only component that — because we're not using `opentelemetry-appender-tracing` — must be written by hand. Approximately 50-70 lines.

**Depends on**: Task 1 (needs `LoggerProvider` type).

**Acceptance Criteria**:

- `OtelLogLayer` implements `tracing_subscriber::Layer<S>` for `S: Subscriber + for<'a> LookupSpan<'a>`
- `on_event()` extracts severity by mapping `tracing::Level` → `opentelemetry::Severity`:
  - `ERROR` → `Severity::Error`
  - `WARN` → `Severity::Warn`
  - `INFO` → `Severity::Info`
  - `DEBUG` → `Severity::Debug`
  - `TRACE` → `Severity::Trace`
- Extracts the event's message (via `tracing::field::display` or visiting fields), uses it as the log record body
- Extracts all event fields as OTel log record attributes
- If the event occurs inside an active tracing span, extracts `trace_id` and `span_id` and attaches them as log attributes
- `OtelLogLayer` is constructed with an `opentelemetry_sdk::logs::Logger` instance
- Compiles alongside `setup.rs` and the dependency set from Task 1

**Test expectations**: Not yet — tested in Task 4 with in-memory exporter.

**Files to create**:

- `poc-otel/src/log_layer.rs` — the `OtelLogLayer` implementation

**Rollout**: Same as Task 1 — throwaway crate.

---

### Task 3: Wire traces and metrics, compose subscriber

**Summary**: Write `traces.rs` and `metrics.rs`, then compose all four subscribers (fmt, OTel trace, OTel log, EnvFilter) in `main.rs`. Wire the signal emission timeline from plan §3.4.

**Depends on**: Task 1 (setup.rs), Task 2 (log_layer.rs).

**Acceptance Criteria**:

- **Traces** (`traces.rs`):
  - Uses `tracing::info_span!()` to create a root span `process_batch`
  - Creates child span `fetch_users` with attributes `db.table="users"`, `db.system="postgres"`
  - Records a span event `cache_miss` inside `fetch_users`
  - Creates child span `send_notifications` with attribute `notification.type="email"`
  - Sets root span status to `Ok`
  - No explicit `opentelemetry::trace::Tracer` calls — all spans go through `tracing` macros
- **Metrics** (`metrics.rs`):
  - Uses `opentelemetry_sdk::metrics::Meter` directly (not tracing bridge)
  - Creates counter `requests_total` with attributes `endpoint`, `status`
  - Creates histogram `request_duration_ms` with attribute `endpoint`
  - Creates gauge `active_connections` with attribute `pool`
  - All instruments record values matching plan §3.4 timeline
- **Subscriber composition** (`main.rs`):
  - Calls `setup::init_otel()` then composes `fmt::Layer()`, tracing-opentelemetry layer, `OtelLogLayer`, and `EnvFilter` in a `Registry`
  - Stdout logging and OTel export both work from the same `tracing::info!()` call
- **Signal emission** (`main.rs`):
  - Emits the full timeline from plan §3.4: span tree, metric instruments, and three log records
  - Prints `"Open http://localhost:8000 to view telemetry"` before exiting
  - Drops `ShutdownGuard` to flush all providers on exit

**Test expectations**: Not yet — Task 4 covers all tests. Verify by running `cargo run` (will fail export since no viewer running, but must not crash — NFR1).

**Files to create**:

- `poc-otel/src/traces.rs` — span construction functions
- `poc-otel/src/metrics.rs` — instrument creation and recording
- `poc-otel/src/main.rs` — update from placeholder to full orchestration

**Rollout**: Same as Tasks 1-2 — throwaway crate.

---

### Task 4: Write offline in-memory unit tests

**Summary**: Write `tests/in_memory.rs` covering all seven test scenarios from plan §4.1. Tests use `opentelemetry_sdk`'s `testing` feature with in-memory exporters — no OTLP receiver required.

**Depends on**: Task 3 (all signal modules exist).

**Acceptance Criteria**:
All tests pass with `cargo test` (no network, no external processes):

| Test                           | What it asserts                                                                                                          | Spec req      |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------ | ------------- |
| `test_trace_emission`          | Span names match, parent-child correct, attributes present, status set                                                   | FR1           |
| `test_metric_emission`         | Counter=1, histogram count=1 with value 42, gauge=7                                                                      | FR2           |
| `test_log_emission`            | Log severity, body, and attributes match expected values                                                                 | FR3           |
| `test_trace_log_correlation`   | Log record's `trace_id` == active span's `trace_id`                                                                      | §5 edge cases |
| `test_resource_attributes`     | All three signals carry same `service.name` from shared Resource                                                         | FR6           |
| `test_dual_output`             | Stdout captured AND log record in exporter from single `tracing::info!()` call                                           | FR5, NFR5     |
| `test_no_crash_on_no_receiver` | OTLP exporter pointed at unreachable port, all signal functions run, process exits 0, stderr contains `warn` not `panic` | NFR1          |

**Files to create**:

- `poc-otel/tests/in_memory.rs` — all seven tests
- `poc-otel/tests/mod.rs` (if needed)

**Rollout**: Same — throwaway crate. Tests run in CI without special setup.

---

### Task 5: Manual integration test with otel-desktop-viewer

**Summary**: Run the PoC end-to-end with `otel-desktop-viewer` and visually verify all three signals arrive correctly. Measure binary size impact and compile time.

**Depends on**: Task 4 (tests pass).

**Acceptance Criteria**:

- **Integration test**: Run `otel-desktop-viewer` via Docker, run `cargo run` with `OTEL_SERVICE_NAME="test-poc"`. All three tabs (traces, metrics, logs) contain data in `otel-desktop-viewer`.
  - Traces tab: `process_batch` root span with children `fetch_users` and `send_notifications`. Attributes visible. `cache_miss` event visible.
  - Metrics tab: `requests_total` counter appears, `request_duration_ms` histogram appears, `active_connections` gauge appears.
  - Logs tab: Three log records (info, warn, error) with correct severity and attributes. Log records within spans carry matching trace IDs.
- **Failure test**: Run `cargo run` with no `otel-desktop-viewer` running. Process exits with code 0. Stderr does not contain `panic`.
- **Env var test**: Run with `OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:19999 cargo run` — process does not crash. If a proxy listener is set up on 19999, data arrives at the alternative endpoint.
- **Binary size measurement**: Measure switchboard release binary size before and after (baseline now, then after Task 6). Record the delta. (NFR2 — must be < 2MB.)
- **Compile time measurement**: Measure full workspace cold build time before and after. Record the delta. (NFR2 — must be < 30%.)

**Rollout**: Nothing to roll back — this is a verification task.

---

## Baseline Measurements (2026-07-16)

| Binary                            | Profile | Size   | Build Time    |
| --------------------------------- | ------- | ------ | ------------- |
| `agentkit-switchboard` (pre-OTel) | release | 13 MB  | 15.52s (warm) |
| `poc-otel` (standalone OTel)      | release | 5.5 MB | 30.90s (warm) |

**NFR2 target**: Delta < 2 MB, compile time increase < 30%.

**Note**: The switchboard warm build time is distorted because PoC deps were already compiled in the same workspace. True cold build will be measured after Task 6 integration.

## Implementation Sequence

```mermaid
flowchart LR
    setupRs[Task 1: setup.rs]
    logLayer[Task 2: log_layer.rs]
    tracesMetricsMain[Task 3: traces.rs + metrics.rs + main.rs]
    tests[Task 4: tests/in_memory.rs]
    verification[Task 5: manual verification]

    setupRs --> logLayer --> tracesMetricsMain --> tests --> verification
```

Tasks 1-5 are sequential within the PoC crate. The switchboard integration that Task 6 previously
described is now owned by `adrs/2026-09-06-switchboard-otel`.
