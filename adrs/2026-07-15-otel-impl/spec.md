---
status: draft
created: 2026-07-15
updated: 2026-07-15
author: adrian
decision: pending
---

# Specification: OpenTelemetry Implementation for Rust Services

## 1. Problem

The project uses `tracing` + `tracing-subscriber` for log output. This gives unstructured text to stdout with no ability to:

- Correlate events across service boundaries (no trace context propagation)
- Measure request durations or operation latencies (no metrics)
- Export structured telemetry to a backend for analysis
- Filter or query on structured attributes beyond basic log levels

Observability decisions made now affect every crate in the workspace. The wrong choice couples all services to a single exporter or locks us into a protocol that doesn't fit local development.

**Need**: Evaluate the Rust OpenTelemetry ecosystem and produce at least one proof-of-concept that demonstrates sending traces, metrics, and logs to `otel-desktop-viewer` so we can visually confirm the data looks correct before committing to a project-wide integration strategy.

**Business value**: Ship once with working observability rather than retrofitting later.

## 2. Tasks to Complete

### 2.1 Build a PoC binary that emits all three OTel signals to otel-desktop-viewer

Build a standalone throwaway binary that emits a trace tree, a counter + histogram + gauge, and structured log records. Verify by running `otel-desktop-viewer` and checking all three tabs (traces, metrics, logs) contain the expected data.

### 2.2 Verify the app survives with no collector running

The PoC must not crash when no OTLP receiver is available. Exporter errors must be logged as warnings, not panics. Confirm that when the collector starts later, subsequent runs export normally.

### 2.3 Add OTel to agentkit-switchboard without rewriting existing tracing calls

Integrate OTel into the switchboard crate by adding subscriber layers, not by changing individual `tracing::info!()` / `tracing::warn!()` / `tracing::debug!()` call sites. Add manual metric instruments at the route handler boundary. Verify that existing stdout logging still works alongside OTel export.

## 3. Functional Requirements

### FR1: Trace Emission

The system must emit OTel spans representing timed operations with parent-child relationships.

**Acceptance Criteria**:
- Spans can be created with a name and automatic start/end timestamps
- Child spans form a parent-child tree visible in a trace viewer
- Spans carry user-defined attributes (string, int, float, bool values)
- Span status can be set to Ok or Error with an optional description
- Mid-span events (timestamped annotations with name and attributes) can be recorded
- Existing `tracing` span macros can produce OTel spans without code changes at individual call sites

### FR2: Metric Emission

The system must emit OTel metrics of types counter, histogram, and gauge.

**Acceptance Criteria**:
- A counter can be incremented with optional attributes
- A histogram can record observations with optional attributes
- A gauge can be set to a value with optional attributes
- All three metric types are visible in otel-desktop-viewer's metrics view

### FR3: Log Emission

The system must emit structured OTel log records.

**Acceptance Criteria**:
- Log records carry a timestamp, severity level, message body, and attributes
- Existing `tracing` log macros produce OTel log records without code changes at individual call sites
- Logs are visible in otel-desktop-viewer's logs view with filterable attributes
- Log records emitted inside a span carry that span's trace ID and span ID

### FR4: OTLP Export

All three signals must be exportable via OTLP to a local receiver.

**Acceptance Criteria**:
- Traces, metrics, and logs are all exported over OTLP
- The OTLP endpoint is configurable via environment variable
- Exported data is visible in otel-desktop-viewer on all three tabs

### FR5: No Rewrite of Existing Instrumentation

Existing `tracing` span and event macros must map to OTel signals without rewriting individual call sites.

**Acceptance Criteria**:
- `tracing::info_span!()` creates an OTel span with matching attributes
- `tracing::info!()` / `tracing::warn!()` etc. create OTel log records with matching attributes
- Tracing span nesting maps to OTel span parent-child relationships
- Stdout logging output and OTel export both work simultaneously from the same `tracing` call

### FR6: Consistent Service Identity

All exported signals must carry consistent identifying attributes.

**Acceptance Criteria**:
- Service name is set once and appears on every span, metric, and log
- Service version is set once and appears on every signal
- Resource attributes are shared across all three signals from a single configuration point

## 4. Non-Functional Requirements

### NFR1: No Crash on Exporter Failure

If the OTLP receiver is unreachable, the application continues operating. Telemetry data loss is acceptable; application crash is not.

**Acceptance Criteria**:
- Export failures are non-fatal (logged as warnings, not panics)
- Application produces correct output even when no OTLP receiver is running

### NFR2: Dependency Overhead Within Bounds

The OTel dependency chain must not bloat compile times or binary size unnecessarily.

**Acceptance Criteria**:
- Adding OTel to an existing crate increases its stripped release binary by no more than 2MB
- Full workspace build time with OTel dependencies increases by no more than 30% (cold build)

### NFR3: Configurable via Standard Env Vars

OTel SDK configuration must follow the OpenTelemetry environment variable specification.

**Acceptance Criteria**:
- OTLP endpoint is set via the standard env var
- Service name resource attribute is set via the standard env var
- Protocol (HTTP/protobuf vs gRPC) is set via the standard env var

### NFR4: Testable Without a Collector

OTel instrumentation must be testable without a running OTLP receiver.

**Acceptance Criteria**:
- Tests can assert span names, attributes, status, and parent-child relationships in-memory
- Tests can assert counter values and histogram distributions in-memory
- Tests do not require `otel-desktop-viewer` or any external process

### NFR5: Dual Output During Transition

Human-readable stdout logs and OTel signals must work simultaneously.

**Acceptance Criteria**:
- `tracing_subscriber::fmt()` output continues alongside OTel export
- A single `tracing::info!()` call produces both a stdout log line and an OTel log record
- The `RUST_LOG` env var continues to control stdout log filtering independent of OTel configuration

## 5. Edge Cases

- **Collector unavailable at startup**: Buffered data is dropped oldest-first when the in-memory buffer fills. When the collector appears, new exports succeed automatically. The application does not stall waiting for export.
- **Collector disappears mid-run**: Same behaviour as above — exports fail silently, application continues.
- **High-cardinality metric attributes**: Dynamic values (user IDs, request IDs, session IDs) must not be used as metric attributes. They belong on spans and logs only. This prevents memory pressure and backend query degradation.
- **Graceful shutdown**: On exit, buffered telemetry must be flushed before the process terminates. Shutdown must have a configurable timeout after which remaining data is dropped.
- **Trace-log correlation**: Log records emitted within a span must carry that span's trace ID and span ID so logs can be correlated to traces in the viewer.

## 6. Out of Scope

- gRPC/tonic transport for OTLP (start with HTTP/protobuf; add later if needed)
- Production OTel configuration (sampling, tail-based sampling, custom processors)
- Distributed trace propagation across service boundaries (only one service today)
- Prometheus metrics endpoint (OTLP covers metrics for now)
- OTel collector deployment (otel-desktop-viewer is the collector for local dev)
- Semantic conventions conformance audit (follow conventions where applicable; exhaustive audit deferred)
- Integration into all workspace crates (this ADR produces the foundation; each crate adopts OTel in its own cycle)
