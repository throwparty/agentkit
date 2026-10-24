//! The ACP v2 server: the per-connection handler builder. v2 is a sibling of
//! `acp::v1`; both share orchestration through `acp::common` and neither may
//! import the other.

pub mod capabilities;
pub mod initialize;
pub mod server;

pub(crate) use server::agent;
