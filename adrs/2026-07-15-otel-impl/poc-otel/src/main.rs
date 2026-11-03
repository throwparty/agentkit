mod log_layer;
mod metrics;
mod setup;
mod traces;

use opentelemetry::logs::LoggerProvider;
use opentelemetry::trace::TracerProvider;

#[tokio::main]
async fn main() {
    let setup = setup::init_otel().expect("failed to initialise OTel SDK");

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::Registry;

    let env_filter = tracing_subscriber::EnvFilter::try_from_env("OTEL_LOG_LEVEL")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_level(true);

    let tracer = setup.tracer_provider.tracer("poc-otel");
    let otel_trace_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    let otel_log_layer = log_layer::OtelLogLayer::new(setup.logger_provider.logger("poc-otel"));

    Registry::default()
        .with(env_filter)
        .with(fmt_layer)
        .with(otel_trace_layer)
        .with(otel_log_layer)
        .init();

    tracing::info!("PoC OTel initialised — emitting signals");

    traces::emit_traces();
    metrics::emit_metrics(&setup);

    tracing::info!("Signals emitted. Shutting down OTel SDK.");

    drop(setup);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    println!("PoC OTel completed successfully.");
}
