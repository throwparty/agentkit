//! Telemetry: structured logging and OpenTelemetry spans, per the
//! switchboard-otel pattern.
//!
//! All harness diagnostics go to stderr, never stdout — on the stdio
//! transport stdout is the ACP JSON-RPC wire and must stay clean. The
//! OTLP exporter initialises when `OTEL_EXPORTER_OTLP_ENDPOINT` is set
//! (or `OTEL_TRACES_EXPORTER` names an exporter); otherwise the fmt
//! layer alone runs. Credentials never appear in spans or logs: they
//! are `secrecy::SecretString` at the type level and tool arguments are
//! redacted to their size.

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::SdkTracerProvider as TracerProvider;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

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

/// The diagnostics writer: stderr, always. Stdout is the ACP wire on
/// the stdio transport.
pub fn diagnostics_writer() -> std::io::Stderr {
    std::io::stderr()
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

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::{Span as _, TraceContextExt as _, Tracer as _};
    use opentelemetry_sdk::trace::InMemorySpanExporter;

    /// A scoped tracer: the in-memory exporter-backed provider, without
    /// touching the global state (tests run in parallel).
    fn scoped_tracer() -> (
        InMemorySpanExporter,
        opentelemetry_sdk::trace::Tracer,
        TracerProvider,
    ) {
        let exporter = InMemorySpanExporter::default();
        let provider = TracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let tracer = provider.tracer("agentkit-tackle-test");
        (exporter, tracer, provider)
    }

    #[test]
    fn spans_are_present_and_hierarchical() {
        let (exporter, tracer, provider) = scoped_tracer();

        // The turn span with a tool-call child and a usage child: the
        // shapes the ACP layer's turn path creates.
        let mut turn = tracer.start(TURN_SPAN);
        // The children's parent context: the turn span's identity (a
        // TestSpan carries it without owning the span).
        let turn_cx = opentelemetry::Context::current().with_span(
            opentelemetry::testing::trace::TestSpan(turn.span_context().clone()),
        );
        for name in [TOOL_CALL_SPAN, USAGE_SPAN] {
            let mut child = tracer
                .span_builder(name)
                .start_with_context(&tracer, &turn_cx);
            child.end();
        }
        turn.end();
        provider.force_flush().unwrap();

        let spans = exporter.get_finished_spans().unwrap();
        let names: Vec<String> = spans.iter().map(|span| span.name.to_string()).collect();
        assert!(names.contains(&TURN_SPAN.to_owned()), "{names:?}");
        assert!(names.contains(&TOOL_CALL_SPAN.to_owned()), "{names:?}");
        assert!(names.contains(&USAGE_SPAN.to_owned()), "{names:?}");

        // Hierarchy: the children share the turn span's trace and name
        // it as their parent.
        let turn_span = spans
            .iter()
            .find(|span| span.name.as_ref() == TURN_SPAN)
            .unwrap();
        for name in [TOOL_CALL_SPAN, USAGE_SPAN] {
            let child = spans
                .iter()
                .find(|span| span.name.as_ref() == name)
                .unwrap();
            assert_eq!(
                child.span_context.trace_id(),
                turn_span.span_context.trace_id()
            );
            assert_ne!(child.parent_span_id, opentelemetry::trace::SpanId::INVALID);
        }
    }

    #[test]
    fn tool_arguments_are_redacted_to_size() {
        // The span's construction accepts only the tool name and the
        // arguments' byte size: argument VALUES have no path into the
        // attributes at all — the redaction is structural.
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::sink)
                .with_ansi(false),
        );
        tracing::subscriber::with_default(subscriber, || {
            let span = tool_call_span("mcp.echo.echo", 42);
            assert!(span.id().is_some());
        });
    }

    #[test]
    fn credentials_are_redacted_at_the_type_level() {
        let secret = secrecy::SecretString::new("super-secret-token".to_owned().into());
        // The type-level redaction: Debug never shows the value.
        let debugged = format!("{secret:?}");
        assert!(!debugged.contains("super-secret-token"));

        // Exposed only where the auth header is built.
        {
            use secrecy::ExposeSecret as _;
            assert_eq!(secret.expose_secret(), "super-secret-token");
        }
    }
}
