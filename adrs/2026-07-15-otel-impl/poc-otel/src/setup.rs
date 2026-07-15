use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::resource::{
    EnvResourceDetector, SdkProvidedResourceDetector, TelemetryResourceDetector,
};
use opentelemetry_sdk::Resource;
use std::time::Duration;

pub struct OtelSetup {
    pub tracer_provider: opentelemetry_sdk::trace::SdkTracerProvider,
    pub meter_provider: opentelemetry_sdk::metrics::SdkMeterProvider,
    pub logger_provider: opentelemetry_sdk::logs::SdkLoggerProvider,
    #[allow(dead_code)]
    pub guard: ShutdownGuard,
}

pub struct ShutdownGuard {
    tracer_provider: Option<opentelemetry_sdk::trace::SdkTracerProvider>,
    meter_provider: Option<opentelemetry_sdk::metrics::SdkMeterProvider>,
    logger_provider: Option<opentelemetry_sdk::logs::SdkLoggerProvider>,
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        if let Some(tp) = self.tracer_provider.take() {
            if let Err(e) = tp.shutdown() {
                tracing::warn!(error = %e, "tracer provider shutdown failed");
            }
        }
        if let Some(mp) = self.meter_provider.take() {
            if let Err(e) = mp.shutdown() {
                tracing::warn!(error = %e, "meter provider shutdown failed");
            }
        }
        if let Some(lp) = self.logger_provider.take() {
            if let Err(e) = lp.shutdown() {
                tracing::warn!(error = %e, "logger provider shutdown failed");
            }
        }
    }
}

fn build_resource() -> Resource {
    let service_name = std::env::var("OTEL_SERVICE_NAME")
        .unwrap_or_else(|_| "poc-otel".to_string());

    Resource::builder_empty()
        .with_detector(Box::new(SdkProvidedResourceDetector))
        .with_detector(Box::new(EnvResourceDetector::new()))
        .with_detector(Box::new(TelemetryResourceDetector))
        .with_attribute(KeyValue::new("service.name", service_name))
        .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
        .build()
}

pub fn init_otel() -> Result<OtelSetup, Box<dyn std::error::Error>> {
    let resource = build_resource();

    let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(
            opentelemetry_otlp::SpanExporter::builder()
                .with_http()
                .with_timeout(Duration::from_secs(10))
                .build()?,
        )
        .with_resource(resource.clone())
        .build();

    let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_timeout(Duration::from_secs(10))
        .build()?;

    let reader = opentelemetry_sdk::metrics::PeriodicReader::builder(metric_exporter)
        .with_interval(Duration::from_secs(60))
        .build();

    let meter_provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(resource.clone())
        .build();

    let logger_provider = opentelemetry_sdk::logs::SdkLoggerProvider::builder()
        .with_batch_exporter(
            opentelemetry_otlp::LogExporter::builder()
                .with_http()
                .with_timeout(Duration::from_secs(10))
                .build()?,
        )
        .with_resource(resource.clone())
        .build();

    let guard = ShutdownGuard {
        tracer_provider: Some(tracer_provider.clone()),
        meter_provider: Some(meter_provider.clone()),
        logger_provider: Some(logger_provider.clone()),
    };

    Ok(OtelSetup {
        tracer_provider,
        meter_provider,
        logger_provider,
        guard,
    })
}
