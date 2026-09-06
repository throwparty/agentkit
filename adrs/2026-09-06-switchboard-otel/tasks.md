# Switchboard: OpenTelemetry Integration

## Tasks

### T-001: [x] Add OTel dependencies

Add the PoC-validated OTel crates to crates/agentkit-switchboard/Cargo.toml: opentelemetry 0.32 (trace, metrics, logs), opentelemetry_sdk 0.32 (trace, metrics, logs, testing, rt-tokio), opentelemetry-otlp 0.32 (http-proto, reqwest-blocking-client, trace, metrics, logs), opentelemetry-stdout 0.32, tracing-opentelemetry 0.33. Enable tracing's attributes feature and tracing-subscriber's registry feature. No feature gating of the OTel deps.


| Field | Value |
|-------|-------|
| Success Criteria | cargo build -p agentkit-switchboard succeeds with the new dependencies; no feature flags gate them. |
| Complexity | 🟢 Low |
| Effort | 1h |
| Depends On |  |
| References | otel-subscriber, dependency-overhead |

### T-002: Create otel module with exporter selection

Create src/otel/mod.rs: TelemetryConfig::from_env() reads OTEL_TRACES_EXPORTER, OTEL_METRICS_EXPORTER, OTEL_LOGS_EXPORTER per signal (case-insensitive otlp | console | none; unset selects none; unknown values log a warning and select none); build_resource() sets service.name from OTEL_SERVICE_NAME (fallback agentkit-switchboard) and service.version; per-signal providers use upstream exporters (opentelemetry-otlp for otlp, opentelemetry-stdout for console) with batch processors for traces/logs and a PeriodicReader for metrics; ShutdownGuard flushes all configured providers on drop.


| Field | Value |
|-------|-------|
| Success Criteria | With OTEL_TRACES_EXPORTER=console the module builds a console span exporter; with otlp it builds an OTLP HTTP exporter; with unset or unknown values it builds no tracer provider. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On |  |
| References | exporter-selection, console-exporter, otlp-export, graceful-shutdown |

### T-003: Port OtelLogLayer

Copy the PoC log_layer.rs to src/otel/log_layer.rs unchanged: maps tracing::Level to Severity, event message to the log body, and event fields to attributes; emits via an SdkLogger.


| Field | Value |
|-------|-------|
| Success Criteria | src/otel/log_layer.rs compiles; existing tracing macros emit OTel log records without per-site changes. |
| Complexity | 🟢 Low |
| Effort | 1h |
| Depends On | T-001 |
| References | otel-subscriber, logs-and-stdout |

### T-004: Compose subscriber and wire main.rs

init_telemetry(log_level) sets global providers for the enabled signals, then composes tracing_subscriber::registry() with fmt::layer(), tracing-opentelemetry::layer() (when traces enabled), OtelLogLayer (when logs enabled), and EnvFilter (RUST_LOG override; fallback {crate_name}={log_level}). On init failure it falls back to the current fmt-only subscriber. main.rs replaces tracing_subscriber::fmt().init() with otel::init_telemetry(&cli.log_level) and holds the ShutdownGuard for the process lifetime.


| Field | Value |
|-------|-------|
| Success Criteria | A single tracing::info!() produces both stdout output and an OTel log record when logs are enabled; with no exporter selected the switchboard starts and logs normally; the process flushes providers on exit. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-002, T-003 |
| References | otel-subscriber, dual-output, no-crash-without-collector, default-no-export |

### T-005: Add metrics and record them

Create src/otel/metrics.rs exposing a cached Metrics struct via the global meter: switchboard.http.requests counter (method, path, status_code) and switchboard.provider.latency histogram (provider_identity, model_name). Record the histogram in proxy_handler after the upstream forward (using the existing latency_ms) and the counter from a middleware after each HTTP response. No high-cardinality attributes.


| Field | Value |
|-------|-------|
| Success Criteria | Both instruments compile and record; a metrics viewer shows switchboard.http.requests and switchboard.provider.latency with the specified attributes; no dynamic IDs on metric attributes. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-002 |
| References | http-request-metric, provider-latency-metric, no-high-cardinality-metrics |

### T-006: Add hot-path spans

Add #[tracing::instrument] at INFO level to proxy_handler, forwarder::forward_request, registry::get_states, registry::record_response, routes::log_routing_event, and session::sqlite update_tokens, assign, lookup so each request produces a phase-span tree.


| Field | Value |
|-------|-------|
| Success Criteria | A single request produces a root proxy_handler span with child spans for routing, upstream forward, quota recording, and SQLite writes whose durations attribute time to each phase. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-004 |
| References | hot-path-spans, phase-trace-tree |

### T-007: Write hermetic tests and verify

Add switchboard tests using opentelemetry_sdk in-memory exporters asserting: a request produces the phase-span tree; switchboard.provider.latency and switchboard.http.requests are recorded with correct attributes; a tracing::info! produces both an OTel log record and stdout. Run cargo clippy --all-targets -- -D warnings and cargo test. Manually verify with OTEL_TRACES_EXPORTER=console that phase spans print to stdout.


| Field | Value |
|-------|-------|
| Success Criteria | cargo test passes; clippy -D warnings is clean; console selection prints spans/metrics/logs to stdout. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-004, T-005, T-006 |
| References | phase-trace-tree, metrics-visible, logs-and-stdout, console-selection, quality-gates, default-no-export |

