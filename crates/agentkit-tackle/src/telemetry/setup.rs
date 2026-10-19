//! The tracing subscriber setup, the OTel exporter wiring, and the span constructors.

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::SdkTracerProvider as TracerProvider;
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialises the tracing subscriber: env-filtered (`RUST_LOG`), writing
/// to stderr, with the OTel trace layer when the OTLP endpoint is
/// configured. Returns the shutdown guard flushing spans on drop.
pub fn init() -> TelemetryGuard {
    let config = TelemetryConfig::from_env();
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    if !config.any_enabled() {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init();
        return TelemetryGuard::default();
    }

    // The switchboard-otel pattern: build the OTLP tracer provider and
    // stack the tracing-opentelemetry layer over the stderr fmt layer.
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .build();
    let provider = match exporter {
        Ok(exporter) => TracerProvider::builder()
            .with_batch_exporter(exporter)
            .build(),
        Err(error) => {
            eprintln!("tackle: OTel exporter init failed: {error}");
            TracerProvider::builder().build()
        }
    };
    let tracer = provider.tracer("agentkit-tackle");
    opentelemetry::global::set_tracer_provider(provider.clone());

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(filter),
        )
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .init();

    TelemetryGuard {
        provider: Some(provider),
    }
}

/// Which exporters are on, per the switchboard-otel env contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TelemetryConfig {
    pub traces: bool,
}

impl TelemetryConfig {
    pub fn from_env() -> Self {
        let endpoint_set = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok();
        let traces = match std::env::var("OTEL_TRACES_EXPORTER") {
            Ok(value) => {
                let value = value.trim().to_ascii_lowercase();
                value == "otlp"
            }
            Err(_) => endpoint_set,
        };
        Self { traces }
    }

    pub fn any_enabled(&self) -> bool {
        self.traces
    }
}

/// Flushes the tracer provider on drop.
#[derive(Default)]
pub struct TelemetryGuard {
    provider: Option<TracerProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(provider) = &self.provider {
            let _ = provider.force_flush();
        }
    }
}

/// The span names tackle emits.
pub const TURN_SPAN: &str = "tackle.turn";
pub const TOOL_CALL_SPAN: &str = "tackle.tool_call";
pub const USAGE_SPAN: &str = "tackle.usage";

/// The turn's span: one per client prompt turn.
pub fn turn_span(session: &str, actor: &str, model: &str) -> tracing::Span {
    tracing::info_span!(TURN_SPAN, session = session, actor = actor, model = model,)
}

/// A tool call's span, the turn span's child. Tool arguments are
/// redacted to their size — never their values.
pub fn tool_call_span(tool: &str, arguments_bytes: usize) -> tracing::Span {
    tracing::info_span!(
        TOOL_CALL_SPAN,
        tool = tool,
        arguments_bytes = arguments_bytes,
    )
}

/// The token usage, recorded as a span: the counts, never the content.
pub fn usage_span(session: &str, input_tokens: u64, output_tokens: u64) -> tracing::Span {
    tracing::info_span!(
        USAGE_SPAN,
        session = session,
        input_tokens = input_tokens,
        output_tokens = output_tokens,
    )
}
