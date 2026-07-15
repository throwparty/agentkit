use std::sync::Once;

use opentelemetry::logs::{LogRecord, Logger, LoggerProvider, Severity};
use opentelemetry::metrics::MeterProvider;
use opentelemetry::trace::{Span, TraceContextExt, Tracer, TracerProvider};
use opentelemetry::KeyValue;
use opentelemetry_sdk::logs::{InMemoryLogExporter, SimpleLogProcessor};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SimpleSpanProcessor};
use opentelemetry_sdk::Resource;
use poc_otel::log_layer::OtelLogLayer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Registry;

static INIT: Once = Once::new();

fn test_resource() -> Resource {
    Resource::builder_empty()
        .with_attribute(KeyValue::new("service.name", "test-poc"))
        .build()
}

#[tokio::test]
async fn test_trace_emission() {
    let exporter = InMemorySpanExporter::default();
    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_span_processor(SimpleSpanProcessor::new(exporter.clone()))
        .with_resource(test_resource())
        .build();

    let tracer = provider.tracer("test");
    let root = tracer
        .span_builder("process_batch")
        .with_kind(opentelemetry::trace::SpanKind::Internal)
        .start(&tracer);
    let root_cx = opentelemetry::Context::current_with_span(root);

    let mut child = tracer.span_builder("fetch_users").start(&tracer);
    child.set_attribute(KeyValue::new("db.table", "users"));
    child.set_attribute(KeyValue::new("db.system", "postgres"));
    child.end();

    let mut child2 = tracer.span_builder("send_notifications").start(&tracer);
    child2.set_attribute(KeyValue::new("notification.type", "email"));
    child2.end();

    root_cx.span().set_status(opentelemetry::trace::Status::Ok);
    root_cx.span().end();
    provider.force_flush().unwrap();

    let spans = exporter.get_finished_spans().unwrap();
    assert_eq!(spans.len(), 3, "expected 3 spans, got {}", spans.len());

    let root_span = spans.iter().find(|s| s.name == "process_batch").unwrap();
    assert!(root_span.span_context.trace_flags().is_sampled());

    let fetch = spans.iter().find(|s| s.name == "fetch_users").unwrap();
    assert!(fetch
        .attributes
        .iter()
        .any(|kv| kv.key.as_str() == "db.table"));

    let notif = spans
        .iter()
        .find(|s| s.name == "send_notifications")
        .unwrap();
    assert!(notif
        .attributes
        .iter()
        .any(|kv| kv.key.as_str() == "notification.type"));
}

#[tokio::test]
async fn test_metric_emission() {
    let exporter = InMemoryMetricExporter::default();
    let reader = PeriodicReader::builder(exporter.clone()).build();
    let provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(test_resource())
        .build();

    let meter = provider.meter("test");
    let counter = meter.u64_counter("requests_total").build();
    let histogram = meter.f64_histogram("request_duration_ms").build();
    let gauge = meter.u64_gauge("active_connections").build();

    counter.add(1, &[KeyValue::new("endpoint", "/batch"), KeyValue::new("status", "success")]);
    counter.add(1, &[KeyValue::new("endpoint", "/batch"), KeyValue::new("status", "error")]);
    histogram.record(42.0, &[KeyValue::new("endpoint", "/batch")]);
    histogram.record(137.5, &[KeyValue::new("endpoint", "/batch")]);
    gauge.record(7, &[KeyValue::new("pool", "upstream")]);

    provider.force_flush().unwrap();
    let metrics = exporter.get_finished_metrics().unwrap();
    assert!(!metrics.is_empty(), "expected metrics");
}

#[tokio::test]
async fn test_log_emission() {
    let exporter = InMemoryLogExporter::default();
    let provider = opentelemetry_sdk::logs::SdkLoggerProvider::builder()
        .with_log_processor(SimpleLogProcessor::new(exporter.clone()))
        .with_resource(test_resource())
        .build();

    let logger = provider.logger("test");
    let mut record = logger.create_log_record();
    record.set_severity_number(Severity::Error);
    record.set_body("test error message".into());
    record.set_observed_timestamp(std::time::SystemTime::now());
    logger.emit(record);

    let _ = provider.force_flush();
    let logs = exporter.get_emitted_logs().unwrap();
    assert_eq!(logs.len(), 1);

    let log = &logs[0];
    assert_eq!(log.record.severity_number(), Some(Severity::Error));
}

#[tokio::test]
async fn test_trace_log_correlation() {
    let span_exporter = InMemorySpanExporter::default();
    let log_exporter = InMemoryLogExporter::default();

    let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_span_processor(SimpleSpanProcessor::new(span_exporter.clone()))
        .with_resource(test_resource())
        .build();

    let logger_provider = opentelemetry_sdk::logs::SdkLoggerProvider::builder()
        .with_log_processor(SimpleLogProcessor::new(log_exporter.clone()))
        .with_resource(test_resource())
        .build();

    let tracer = tracer_provider.tracer("test");
    let logger = logger_provider.logger("test");

    let span = tracer.span_builder("parent").start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    let _guard = cx.attach();

    let mut record = logger.create_log_record();
    record.set_severity_number(Severity::Info);
    record.set_body("inside span".into());
    record.set_observed_timestamp(std::time::SystemTime::now());
    logger.emit(record);

    drop(_guard);

    tracer_provider.force_flush().unwrap();
    let _ = logger_provider.force_flush();

    let spans = span_exporter.get_finished_spans().unwrap();
    let logs = log_exporter.get_emitted_logs().unwrap();

    assert_eq!(spans.len(), 1);
    assert_eq!(logs.len(), 1);

    let span_trace_id = spans[0].span_context.trace_id();
    let log_trace_id = logs[0].record.trace_context().unwrap().trace_id;
    assert_eq!(log_trace_id, span_trace_id);
}

#[tokio::test]
async fn test_resource_attributes() {
    let resource = test_resource();

    let span_exporter = InMemorySpanExporter::default();
    let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_span_processor(SimpleSpanProcessor::new(span_exporter.clone()))
        .with_resource(resource.clone())
        .build();

    let metric_exporter = InMemoryMetricExporter::default();
    let reader = PeriodicReader::builder(metric_exporter.clone()).build();
    let meter_provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(resource.clone())
        .build();

    let log_exporter = InMemoryLogExporter::default();
    let logger_provider = opentelemetry_sdk::logs::SdkLoggerProvider::builder()
        .with_log_processor(SimpleLogProcessor::new(log_exporter.clone()))
        .with_resource(resource.clone())
        .build();

    let tracer = tracer_provider.tracer("test");
    let mut span = tracer.span_builder("test_span").start(&tracer);
    span.end();
    tracer_provider.force_flush().unwrap();

    let meter = meter_provider.meter("test");
    let counter = meter.u64_counter("test_counter").build();
    counter.add(1, &[]);
    meter_provider.force_flush().unwrap();

    let logger = logger_provider.logger("test");
    let mut record = logger.create_log_record();
    record.set_severity_number(Severity::Info);
    record.set_body("test".into());
    logger.emit(record);
    let _ = logger_provider.force_flush();

    let spans = span_exporter.get_finished_spans().unwrap();
    let metrics = metric_exporter.get_finished_metrics().unwrap();
    let logs = log_exporter.get_emitted_logs().unwrap();

    assert!(!spans.is_empty());
    assert!(!metrics.is_empty());
    assert!(!logs.is_empty());
}

#[tokio::test]
async fn test_dual_output() {
    let log_exporter = InMemoryLogExporter::default();
    let logger_provider = opentelemetry_sdk::logs::SdkLoggerProvider::builder()
        .with_log_processor(SimpleLogProcessor::new(log_exporter.clone()))
        .with_resource(test_resource())
        .build();

    INIT.call_once(|| {
        let env_filter = tracing_subscriber::EnvFilter::new("info");
        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_test_writer()
            .with_target(true)
            .with_level(true);
        let log_layer = OtelLogLayer::new(logger_provider.logger("test"));

        Registry::default()
            .with(env_filter)
            .with(fmt_layer)
            .with(log_layer)
            .init();
    });

    tracing::info!("dual output test message");
    let _ = logger_provider.force_flush();

    let logs = log_exporter.get_emitted_logs().unwrap();
    assert!(!logs.is_empty(), "expected log record in exporter");
}

#[tokio::test]
async fn test_no_crash_on_no_receiver() {
    std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://localhost:19999");
    std::env::set_var("OTEL_SERVICE_NAME", "test-crash");

    let setup = poc_otel::setup::init_otel().expect("init_otel should not panic");
    drop(setup);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}
