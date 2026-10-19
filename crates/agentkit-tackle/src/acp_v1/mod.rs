//! ACP server surface: protocol handling via the SDK's callback builders.
//!
//! Handlers are registered per request type and tried in order; shared
//! state rides in `Arc<TackleState>` captured by each callback, and
//! per-connection negotiation state in `Arc<ConnectionNegotiation>`. The
//! stdio transport is the normal launch mode (one process per client
//! connection); HTTP serves many connections per process.
//!
//! This module is declarations and re-exports only: the request handlers
//! live in `server`, the per-connection state in `negotiation`, and the
//! orchestration itself in `crate::agent`.

mod capabilities;
mod commands;
mod config_options;
mod negotiation;
mod notices;
mod permission;
mod replay;
mod server;
mod sink;

pub mod client_tools;
pub mod http;

#[cfg(test)]
mod tests;

pub use capabilities::UnstableFeature;
pub use negotiation::ConnectionNegotiation;
pub use server::run_stdio;
pub use crate::state::{PendingProject, TackleState};
pub(crate) use commands::connect_config_servers;
pub(crate) use server::run_agent_over;
pub(crate) use config_options::{wire_notice, wire_option, wire_options};
pub(crate) use sink::{ConnectionTurnSink, wire_kind};
