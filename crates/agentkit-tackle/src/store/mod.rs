//! Session storage: the turn DAG behind a `SessionStore` with two
//! interchangeable backends — SQLite (durable, WAL, multi-process) and
//! in-memory (tests). Adopted from adrs/2026-06-13-acp-server-session-storage
//! with the tackle schema extensions: turn kind, first_retained_turn_id,
//! per-turn usage deltas, attribution metadata.
//!
//! Ownership leases and context assembly build on this.

mod model;
mod session;
mod sqlite;

#[cfg(test)]
mod assembly;
#[cfg(test)]
mod graph;
#[cfg(test)]
mod memory;
#[cfg(test)]
mod tests;

pub use model::{
    session_actor_value_free, session_model_free, AssembledMessage, ListFilter, Message, MessageId,
    Role, Session, SessionId, SessionKind, StoreError, Turn, TurnId, TurnKind, TurnUsage,
};
pub use session::SessionStore;

#[cfg(test)]
pub(crate) use assembly::assembly_order;
#[cfg(test)]
pub use graph::SessionGraph;
