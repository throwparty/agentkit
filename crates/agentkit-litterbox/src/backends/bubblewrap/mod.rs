//! Bubblewrap as a sandboxing mechanism.
//!
//! The sandbox is a holder process in the pause-container shape: bwrap creates
//! the namespaces and runs a long-lived command there, and every other command
//! joins those namespaces in its own process.

// The mechanisms land in staged tasks and the seam is used from the record
// validation onward, so the incomplete module allows dead code until then.
#![allow(dead_code)]

pub mod backend;
pub mod config;
pub mod construct;
pub mod holder;
pub mod host;
pub mod view;

/// The backend's own name in records and reports.
pub const BACKEND: &str = "bubblewrap";
