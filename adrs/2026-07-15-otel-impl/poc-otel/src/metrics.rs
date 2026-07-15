use crate::setup::OtelSetup;
use opentelemetry::metrics::MeterProvider;
use opentelemetry::KeyValue;

pub fn emit_metrics(setup: &OtelSetup) {
    let meter = setup.meter_provider.meter("poc-otel");

    let requests_total = meter.u64_counter("requests_total").build();
    let request_duration = meter.f64_histogram("request_duration_ms").build();
    let active_connections = meter.u64_gauge("active_connections").build();

    requests_total.add(1, &[KeyValue::new("endpoint", "/batch"), KeyValue::new("status", "success")]);
    requests_total.add(1, &[KeyValue::new("endpoint", "/batch"), KeyValue::new("status", "error")]);

    request_duration.record(42.0, &[KeyValue::new("endpoint", "/batch")]);
    request_duration.record(137.5, &[KeyValue::new("endpoint", "/batch")]);

    active_connections.record(7, &[KeyValue::new("pool", "upstream")]);
}
