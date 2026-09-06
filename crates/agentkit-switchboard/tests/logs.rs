use std::io::Write;
use std::sync::{Arc, Mutex};

use agentkit_switchboard::otel::log_layer::OtelLogLayer;
use opentelemetry::logs::AnyValue;
use opentelemetry::logs::LoggerProvider;
use opentelemetry_sdk::logs::InMemoryLogExporter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Clone)]
struct BufferWriter(Arc<Mutex<String>>);

struct BufferSink(Arc<Mutex<String>>);

impl Write for BufferSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap()
            .push_str(&String::from_utf8_lossy(buf));
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for BufferWriter {
    type Writer = BufferSink;

    fn make_writer(&'a self) -> Self::Writer {
        BufferSink(self.0.clone())
    }
}

fn any_value_str(v: &AnyValue) -> Option<&str> {
    match v {
        AnyValue::String(s) => Some(s.as_str()),
        _ => None,
    }
}

#[test]
fn tracing_event_emits_otel_log_and_stdout() {
    let log_exporter = InMemoryLogExporter::default();
    let logger_provider = opentelemetry_sdk::logs::SdkLoggerProvider::builder()
        .with_simple_exporter(log_exporter.clone())
        .build();
    let logger = logger_provider.logger("agentkit-switchboard");

    let stdout = Arc::new(Mutex::new(String::new()));
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(BufferWriter(stdout.clone()))
        .with_target(false)
        .with_level(false);

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(OtelLogLayer::new(logger))
        .init();

    tracing::info!(key = "val", "hello from test");

    let logs = log_exporter.get_emitted_logs().unwrap();
    assert!(
        logs.iter().any(|log| {
            let body_matches = matches!(
                log.record.body(),
                Some(AnyValue::String(s)) if s.as_str() == "hello from test"
            );
            let attr_matches = log
                .record
                .attributes_iter()
                .any(|(k, v)| k.as_str() == "key" && any_value_str(v) == Some("val"));
            body_matches && attr_matches
        }),
        "tracing::info! should produce an OTel log record with body and attributes"
    );

    let captured = stdout.lock().unwrap().clone();
    assert!(
        captured.contains("hello from test"),
        "fmt layer should also write the event to stdout, got: {captured}"
    );
}