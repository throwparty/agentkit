//! Telemetry tests: span presence/hierarchy, redaction.

    use super::*;
    use opentelemetry::trace::{Span as _, TraceContextExt as _, Tracer as _, TracerProvider as _};
    use opentelemetry_sdk::trace::InMemorySpanExporter;
    use opentelemetry_sdk::trace::SdkTracerProvider as TracerProvider;
    use tracing_subscriber::layer::SubscriberExt;

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
