//! The sandboxing mechanisms this binary can run.
//!
//! The set is compiled in: `backends/registry.rs` is the only place a
//! mechanism is selected or constructed.

pub mod docker;
pub mod registry;
pub mod report;

pub use registry::{AVAILABLE, DEFAULT, Selected, build, introspect, select, selected_name};
