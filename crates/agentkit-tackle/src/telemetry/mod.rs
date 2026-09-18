//! Telemetry: structured logging and OpenTelemetry spans.
//!
//! All harness diagnostics go to stderr, never stdout — on the stdio
//! transport stdout is the ACP JSON-RPC wire and must stay clean.

use tracing_subscriber::EnvFilter;

/// Initialises the tracing subscriber: env-filtered (`RUST_LOG`), writing
/// to stderr.
pub fn init() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
