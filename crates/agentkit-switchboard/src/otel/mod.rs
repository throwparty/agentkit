pub mod log_layer;
pub mod metrics;

use opentelemetry::logs::LoggerProvider;
use opentelemetry::trace::TracerProvider;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::resource::{
    EnvResourceDetector, SdkProvidedResourceDetector, TelemetryResourceDetector,
};
use opentelemetry_sdk::logs::{BatchLogProcessor, BatchConfigBuilder as LogBatchConfigBuilder};
use opentelemetry_sdk::trace::{BatchSpanProcessor, BatchConfigBuilder as TraceBatchConfigBuilder, SdkTracerProvider};
use opentelemetry_sdk::Resource;
use std::time::Duration;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExporterKind {
    Otlp,
    Console,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryConfig {
    pub traces: ExporterKind,
    pub metrics: ExporterKind,
    pub logs: ExporterKind,
}

impl TelemetryConfig {
    pub fn from_env() -> Self {
        let endpoint_set = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok();
        Self {
            traces: or_otlp_if_endpoint("OTEL_TRACES_EXPORTER", endpoint_set),
            metrics: or_otlp_if_endpoint("OTEL_METRICS_EXPORTER", endpoint_set),
            logs: or_otlp_if_endpoint("OTEL_LOGS_EXPORTER", endpoint_set),
        }
    }

    pub fn any_enabled(&self) -> bool {
        self.traces != ExporterKind::None
            || self.metrics != ExporterKind::None
            || self.logs != ExporterKind::None
    }
}

fn or_otlp_if_endpoint(var: &str, endpoint_set: bool) -> ExporterKind {
    match std::env::var(var) {
        Ok(value) => parse_exporter_value(&value, var),
        Err(_) if endpoint_set => ExporterKind::Otlp,
        Err(_) => ExporterKind::None,
    }
}

fn parse_exporter_value(value: &str, var: &str) -> ExporterKind {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "otlp" => ExporterKind::Otlp,
        "console" => ExporterKind::Console,
        "none" | "" => ExporterKind::None,
        other => {
            tracing::warn!(
                var,
                value = other,
                "unrecognised exporter value; disabling exporter"
            );
            ExporterKind::None
        }
    }
}

pub fn init_telemetry(log_level: &str) -> ShutdownGuard {
    let config = TelemetryConfig::from_env();
    if !config.any_enabled() {
        init_fmt_only(log_level);
        return ShutdownGuard::new(empty_providers());
    }

    let providers = match build_providers(&config) {
        Ok(providers) => providers,
        Err(error) => {
            tracing::warn!(%error, "OTel initialisation failed; falling back to stdout logging");
            init_fmt_only(log_level);
            return ShutdownGuard::new(empty_providers());
        }
    };

    if let Some(tp) = &providers.tracer_provider {
        opentelemetry::global::set_tracer_provider(tp.clone());
    }
    if let Some(mp) = &providers.meter_provider {
        opentelemetry::global::set_meter_provider(mp.clone());
    }

    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(format!(
            "{}={}",
            env!("CARGO_CRATE_NAME"),
            log_level
        ))
    });

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_level(true);

    let mut layers: Vec<Box<dyn Layer<tracing_subscriber::Registry> + Send + Sync>> =
        Vec::new();
    layers.push(Box::new(fmt_layer));
    if let Some(tp) = &providers.tracer_provider {
        let tracer = tp.tracer("agentkit-switchboard");
        layers.push(Box::new(tracing_opentelemetry::layer().with_tracer(tracer)));
    }
    if let Some(lp) = &providers.logger_provider {
        let logger = lp.logger("agentkit-switchboard");
        layers.push(Box::new(log_layer::OtelLogLayer::new(logger)));
    }
    layers.push(Box::new(filter));

    tracing_subscriber::registry().with(layers).init();

    ShutdownGuard::new(providers)
}

fn init_fmt_only(log_level: &str) {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                format!("{}={}", env!("CARGO_CRATE_NAME"), log_level).into()
            }),
        )
        .init();
}

fn empty_providers() -> OtelProviders {
    OtelProviders {
        tracer_provider: None,
        meter_provider: None,
        logger_provider: None,
    }
}

pub fn build_resource() -> Resource {
    let service_name = std::env::var("OTEL_SERVICE_NAME")
        .unwrap_or_else(|_| "agentkit-switchboard".to_string());

    Resource::builder_empty()
        .with_detector(Box::new(SdkProvidedResourceDetector))
        .with_detector(Box::new(EnvResourceDetector::new()))
        .with_detector(Box::new(TelemetryResourceDetector))
        .with_attribute(KeyValue::new("service.name", service_name))
        .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
        .build()
}

pub struct OtelProviders {
    pub tracer_provider: Option<SdkTracerProvider>,
    pub meter_provider: Option<SdkMeterProvider>,
    pub logger_provider: Option<opentelemetry_sdk::logs::SdkLoggerProvider>,
}

pub fn build_providers(
    config: &TelemetryConfig,
) -> Result<OtelProviders, Box<dyn std::error::Error>> {
    let resource = build_resource();

    let tracer_provider = match config.traces {
        ExporterKind::None => None,
        _ => Some(build_tracer_provider(config.traces, resource.clone())?),
    };
    let meter_provider = match config.metrics {
        ExporterKind::None => None,
        _ => Some(build_meter_provider(config.metrics, resource.clone())?),
    };
    let logger_provider = match config.logs {
        ExporterKind::None => None,
        _ => Some(build_logger_provider(config.logs, resource)?),
    };

    Ok(OtelProviders {
        tracer_provider,
        meter_provider,
        logger_provider,
    })
}

fn build_tracer_provider(
    kind: ExporterKind,
    resource: Resource,
) -> Result<SdkTracerProvider, Box<dyn std::error::Error>> {
    let mut builder = SdkTracerProvider::builder().with_resource(resource);
    let batch_config = TraceBatchConfigBuilder::default()
        .with_scheduled_delay(Duration::from_secs(30))
        .build();
    match kind {
        ExporterKind::Otlp => {
            let exporter = opentelemetry_otlp::SpanExporter::builder()
                .with_http()
                .with_timeout(Duration::from_secs(10))
                .build()?;
            builder = builder.with_span_processor(
                BatchSpanProcessor::builder(exporter)
                    .with_batch_config(batch_config)
                    .build(),
            );
        }
        ExporterKind::Console => {
            builder = builder.with_span_processor(
                BatchSpanProcessor::builder(opentelemetry_stdout::SpanExporter::default())
                    .with_batch_config(batch_config)
                    .build(),
            );
        }
        ExporterKind::None => unreachable!("caller guards None"),
    }
    Ok(builder.build())
}

fn build_meter_provider(
    kind: ExporterKind,
    resource: Resource,
) -> Result<SdkMeterProvider, Box<dyn std::error::Error>> {
    let mut builder = SdkMeterProvider::builder().with_resource(resource);
    match kind {
        ExporterKind::Otlp => {
            let exporter = opentelemetry_otlp::MetricExporter::builder()
                .with_http()
                .with_timeout(Duration::from_secs(10))
                .build()?;
            let reader = PeriodicReader::builder(exporter)
                .with_interval(Duration::from_secs(60))
                .build();
            builder = builder.with_reader(reader);
        }
        ExporterKind::Console => {
            let reader = PeriodicReader::builder(opentelemetry_stdout::MetricExporter::default())
                .with_interval(Duration::from_secs(60))
                .build();
            builder = builder.with_reader(reader);
        }
        ExporterKind::None => unreachable!("caller guards None"),
    }
    Ok(builder.build())
}

fn build_logger_provider(
    kind: ExporterKind,
    resource: Resource,
) -> Result<opentelemetry_sdk::logs::SdkLoggerProvider, Box<dyn std::error::Error>> {
    let mut builder = opentelemetry_sdk::logs::SdkLoggerProvider::builder().with_resource(resource);
    let batch_config = LogBatchConfigBuilder::default()
        .with_scheduled_delay(Duration::from_secs(30))
        .build();
    match kind {
        ExporterKind::Otlp => {
            let exporter = opentelemetry_otlp::LogExporter::builder()
                .with_http()
                .with_timeout(Duration::from_secs(10))
                .build()?;
            builder = builder.with_log_processor(
                BatchLogProcessor::builder(exporter)
                    .with_batch_config(batch_config)
                    .build(),
            );
        }
        ExporterKind::Console => {
            builder = builder.with_log_processor(
                BatchLogProcessor::builder(opentelemetry_stdout::LogExporter::default())
                    .with_batch_config(batch_config)
                    .build(),
            );
        }
        ExporterKind::None => unreachable!("caller guards None"),
    }
    Ok(builder.build())
}

pub struct ShutdownGuard {
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
    logger_provider: Option<opentelemetry_sdk::logs::SdkLoggerProvider>,
}

impl ShutdownGuard {
    pub fn new(providers: OtelProviders) -> Self {
        Self {
            tracer_provider: providers.tracer_provider,
            meter_provider: providers.meter_provider,
            logger_provider: providers.logger_provider,
        }
    }
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        if let Some(tp) = self.tracer_provider.take() {
            if let Err(error) = tp.shutdown() {
                tracing::warn!(%error, "tracer provider shutdown failed");
            }
        }
        if let Some(mp) = self.meter_provider.take() {
            if let Err(error) = mp.shutdown() {
                tracing::warn!(%error, "meter provider shutdown failed");
            }
        }
        if let Some(lp) = self.logger_provider.take() {
            if let Err(error) = lp.shutdown() {
                tracing::warn!(%error, "logger provider shutdown failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_exporter_value_maps_spec_values() {
        assert_eq!(
            parse_exporter_value("otlp", "OTEL_TRACES_EXPORTER"),
            ExporterKind::Otlp
        );
        assert_eq!(
            parse_exporter_value("OTLP", "OTEL_TRACES_EXPORTER"),
            ExporterKind::Otlp
        );
        assert_eq!(
            parse_exporter_value("  otlp  ", "OTEL_TRACES_EXPORTER"),
            ExporterKind::Otlp
        );
        assert_eq!(
            parse_exporter_value("console", "OTEL_TRACES_EXPORTER"),
            ExporterKind::Console
        );
        assert_eq!(
            parse_exporter_value("none", "OTEL_TRACES_EXPORTER"),
            ExporterKind::None
        );
        assert_eq!(
            parse_exporter_value("", "OTEL_TRACES_EXPORTER"),
            ExporterKind::None
        );
    }

    #[test]
    fn parse_exporter_value_unknown_falls_back_to_none() {
        assert_eq!(
            parse_exporter_value("zipkin", "OTEL_TRACES_EXPORTER"),
            ExporterKind::None
        );
        assert_eq!(
            parse_exporter_value("prometheus", "OTEL_METRICS_EXPORTER"),
            ExporterKind::None
        );
    }

    #[test]
    fn telemetry_config_any_enabled() {
        let none = TelemetryConfig {
            traces: ExporterKind::None,
            metrics: ExporterKind::None,
            logs: ExporterKind::None,
        };
        assert!(!none.any_enabled());

        let mixed = TelemetryConfig {
            traces: ExporterKind::Console,
            metrics: ExporterKind::None,
            logs: ExporterKind::Otlp,
        };
        assert!(mixed.any_enabled());
    }

    #[tokio::test]
    async fn build_providers_console_traces() {
        let config = TelemetryConfig {
            traces: ExporterKind::Console,
            metrics: ExporterKind::None,
            logs: ExporterKind::None,
        };
        let providers = build_providers(&config).expect("console providers should build");
        assert!(providers.tracer_provider.is_some());
        assert!(providers.meter_provider.is_none());
        assert!(providers.logger_provider.is_none());
        drop(ShutdownGuard::new(providers));
    }

    #[tokio::test]
    async fn build_providers_otlp_traces() {
        let config = TelemetryConfig {
            traces: ExporterKind::Otlp,
            metrics: ExporterKind::None,
            logs: ExporterKind::None,
        };
        let providers = build_providers(&config).expect("otlp providers should build");
        assert!(providers.tracer_provider.is_some());
        assert!(providers.meter_provider.is_none());
        assert!(providers.logger_provider.is_none());
        drop(ShutdownGuard::new(providers));
    }

    #[tokio::test]
    async fn build_providers_none_builds_nothing() {
        let config = TelemetryConfig {
            traces: ExporterKind::None,
            metrics: ExporterKind::None,
            logs: ExporterKind::None,
        };
        let providers = build_providers(&config).expect("none config should build");
        assert!(providers.tracer_provider.is_none());
        assert!(providers.meter_provider.is_none());
        assert!(providers.logger_provider.is_none());
    }
}