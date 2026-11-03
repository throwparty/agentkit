//! Docker as a sandboxing mechanism.
//!
//! Only wiring lives here: the backend, its API client, its section and its
//! naming and forwarding helpers are siblings.

pub mod backend;
pub mod compute;
pub mod config;
pub mod construct;
pub(crate) mod error;
pub(crate) mod forwarding;
pub(crate) mod naming;

pub use backend::{BACKEND, DockerSettings};
