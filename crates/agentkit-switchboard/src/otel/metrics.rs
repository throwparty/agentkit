use opentelemetry::metrics::{Counter, Histogram};
use std::sync::OnceLock;

pub struct Metrics {
    pub http_requests: Counter<u64>,
    pub provider_latency: Histogram<f64>,
}

static METRICS: OnceLock<Metrics> = OnceLock::new();

pub fn metrics() -> &'static Metrics {
    METRICS.get_or_init(|| {
        let meter = opentelemetry::global::meter("agentkit-switchboard");
        Metrics {
            http_requests: meter.u64_counter("switchboard.http.requests").build(),
            provider_latency: meter.f64_histogram("switchboard.provider.latency").build(),
        }
    })
}
