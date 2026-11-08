//! Bubblewrap as a sandboxing mechanism.
//!
//! The sandbox is a holder process in the pause-container shape: bwrap creates
//! the namespaces and runs a long-lived command there, and every other command
//! joins those namespaces in its own process.

pub mod backend;
pub mod config;
pub mod construct;

/// The backend's own name in records and reports.
pub const BACKEND: &str = "bubblewrap";
