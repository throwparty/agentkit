# Switchboard: OpenTelemetry Integration

**Status:** implemented  **Created:** 2026-09-06  **Author:** adrian

The switchboard (crates/agentkit-switchboard) has no structured observability: a plain tracing_subscriber::fmt() logger, no spans, no metrics, and no trace context. The OTel PoC (adrs/2026-07-15-otel-impl/poc-otel) validated the toolchain (opentelemetry 0.32, opentelemetry_sdk 0.32, opentelemetry-otlp 0.32, tracing-opentelemetry 0.33) with in-memory tests and otel-desktop-viewer; that ADR is complete. This ADR integrates the validated stack into the switchboard. Exporter selection follows the OTel SDK environment variable specification (OTEL_TRACES_EXPORTER, OTEL_METRICS_EXPORTER, OTEL_LOGS_EXPORTER with values otlp, console, none); the console exporter uses the upstream opentelemetry-stdout crate, and unset variables default to none (no export) so telemetry is fully opt-in. Separately, switchboard per-request latency is suspected to be dominated by synchronous SQLite writes and exclusive RwLock scans on the response path (routes.rs proxy_handler); hot-path spans will make each phase's duration visible so the hypothesis can be confirmed or refuted.


## Problem

The switchboard cannot measure where request time goes (upstream vs SQLite writes vs lock contention) and has no structured telemetry to export. The PoC-validated OTel stack is not integrated into the switchboard.

## Goals

- Integrate the PoC-validated OTel stack into the switchboard
- Emit traces, metrics, and logs without rewriting existing tracing call sites
- Follow the OTel SDK environment variable specification for exporter selection
- Add hot-path span instrumentation exposing per-phase request latency
- Add HTTP request and provider latency metrics

## Non-goals

- gRPC/tonic OTLP transport (HTTP/protobuf only)
- Distributed trace propagation across service boundaries
- Prometheus metrics endpoint
- OTel integration into other workspace crates


## Functional Requirements

### FR-001: OTel Subscriber

The switchboard initialises a layered tracing subscriber (fmt, OTel trace, OTel log, EnvFilter) from an otel module; existing tracing macros emit both stdout and OTel signals without per-site changes

**Slug:** `otel-subscriber`

### FR-002: Exporter Selection

Each signal's exporter is selected via OTEL_TRACES_EXPORTER, OTEL_METRICS_EXPORTER, and OTEL_LOGS_EXPORTER accepting case-insensitive values otlp, console, or none; unset selects none, and unknown values log a warning and select none; selection has a direct code equivalent (TelemetryConfig)

**Slug:** `exporter-selection`

### FR-003: Console Exporter

The console exporter (upstream opentelemetry-stdout) writes spans, metrics, and logs to standard output when selected; only upstream exporter types are used

**Slug:** `console-exporter`

### FR-004: OTLP Export

When otlp is selected, traces, metrics, and logs export over OTLP HTTP/protobuf; endpoint from OTEL_EXPORTER_OTLP_ENDPOINT (fallback http://localhost:4318); service name from OTEL_SERVICE_NAME (fallback agentkit-switchboard)

**Slug:** `otlp-export`

### FR-005: Hot-Path Spans

Each request produces a root span with child spans for routing (get_states), upstream forward, quota recording, and session DB writes, exposing per-phase durations

**Slug:** `hot-path-spans`

### FR-006: Provider Latency Metric

A switchboard.provider.latency histogram with provider_identity and model_name attributes is recorded after each upstream response

**Slug:** `provider-latency-metric`

### FR-007: HTTP Request Metric

A switchboard.http.requests counter with method, path, and status_code attributes is recorded after each HTTP response

**Slug:** `http-request-metric`

## Non-functional Requirements

### NFR-001: No Crash Without Collector

OTel initialisation failure falls back to a fmt-only subscriber; export failures are non-fatal; with no exporter selected the switchboard runs normally

**Slug:** `no-crash-without-collector`

### NFR-002: Dual Output

A single tracing call produces both stdout output and OTel signals; RUST_LOG continues to control stdout filtering

**Slug:** `dual-output`

### NFR-003: No High-Cardinality Metric Attributes

No metric instrument uses user IDs, session IDs, request IDs, or other dynamic values as attributes

**Slug:** `no-high-cardinality-metrics`

### NFR-004: Standard Env Vars

Exporter selection follows the OTel SDK environment variable specification (OTEL_TRACES_EXPORTER, OTEL_METRICS_EXPORTER, OTEL_LOGS_EXPORTER) with spec values otlp, console, and none

**Slug:** `standard-env-vars`

## Acceptance Criteria

### AC-001: Default No Export

With the exporter env vars unset, the switchboard starts, serves requests, and exits cleanly with no telemetry exported

**Slug:** `default-no-export`

### AC-002: Console Selection

Setting OTEL_TRACES_EXPORTER=console prints the per-request phase-span tree to stdout; OTEL_METRICS_EXPORTER=console prints the two metrics; OTEL_LOGS_EXPORTER=console prints log records

**Slug:** `console-selection`

### AC-003: Phase Trace Tree

A single request produces a trace tree whose child spans attribute duration to routing, upstream, quota, and SQLite phases

**Slug:** `phase-trace-tree`

### AC-004: Metrics Visible

switchboard.http.requests and switchboard.provider.latency appear in a metrics viewer with the specified attributes

**Slug:** `metrics-visible`

### AC-005: Logs and Stdout

Existing tracing calls emit OTel log records with severity and attributes while stdout logging continues to work

**Slug:** `logs-and-stdout`

### AC-006: Quality Gates

cargo clippy --all-targets -- -D warnings is clean and cargo test passes

**Slug:** `quality-gates`

## Edge Cases

### EC-001: Collector Unavailable

When the OTLP receiver is unreachable at startup or mid-run, data is buffered and dropped oldest-first; the application continues without stalling

**Slug:** `collector-unavailable`
### EC-002: OTel Init Failure

If OTel initialisation fails, the subscriber falls back to fmt-only and metric instruments become no-ops via the global meter

**Slug:** `otel-init-failure`
### EC-003: Path Cardinality

The path metric attribute stays bounded because switchboard routes are fixed; no dynamic identifiers are used as metric attributes

**Slug:** `path-cardinality`
### EC-004: Unknown Exporter Value

An unrecognised exporter value (e.g. zipkin or prometheus) logs a warning and selects no exporter for that signal

**Slug:** `unknown-exporter-value`
### EC-005: Mixed Signal Selection

Exporter selection is per signal, so traces, metrics, and logs may target different sinks independently

**Slug:** `mixed-signal-selection`

