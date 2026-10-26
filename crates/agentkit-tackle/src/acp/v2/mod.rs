//! The ACP v2 server: the per-connection handler builder. v2 is a sibling of
//! `acp::v1`; both share orchestration through `acp::common` and neither may
//! import the other.

pub mod capabilities;
pub mod commands;
pub mod initialize;
pub mod mcp;
pub mod negotiation;
pub mod notices;
pub mod permission;
pub mod prompt;
pub mod replay;
pub mod server;
pub mod sessions;
pub mod sink;
pub mod wire;

pub(crate) use server::agent;
pub(super) use wire::{send_state, store_session_id, wire_session_id};
