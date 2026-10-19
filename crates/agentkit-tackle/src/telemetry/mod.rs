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
//!
//! This module is declarations and re-exports only.

mod setup;

#[cfg(test)]
mod tests;

pub use setup::*;
