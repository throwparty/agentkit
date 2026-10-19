//! ACP server surface: protocol handling via the SDK's callback builders.
//!
//! Handlers are registered per request type and tried in order; shared
//! state rides in `Arc<TackleState>` captured by each callback, and
//! per-connection negotiation state in `Arc<ConnectionNegotiation>`. The
//! stdio transport is the normal launch mode (one process per client
//! connection); HTTP serves many connections per process.
//!
//! This module is declarations and re-exports only: the builder wiring in
//! `server` delegates to per-method handler modules (`initialize`,
//! `sessions`, `prompt`), the per-connection state lives in `negotiation`,
//! and the orchestration itself in `crate::agent`.

mod capabilities;
mod commands;
mod config_options;
mod initialize;
mod negotiation;
mod notices;
mod permission;
mod prompt;
mod replay;
mod server;
mod sessions;
mod sink;
mod support;

pub mod client_tools;
pub mod http;

#[cfg(test)]
mod tests;

pub use crate::state::{PendingProject, TackleState};
pub use capabilities::UnstableFeature;
pub(crate) use commands::connect_config_servers;
pub(crate) use config_options::{wire_notice, wire_option, wire_options};
pub use negotiation::ConnectionNegotiation;
pub(crate) use server::run_agent_over;
pub use server::run_stdio;
pub(crate) use sink::{ConnectionTurnSink, wire_kind};
