//! The sandboxing mechanisms this binary can run.
//!
//! The set is compiled in: `backends/registry.rs` is the only place a
//! mechanism is selected or constructed.

pub(crate) mod docker;
pub mod registry;

pub use registry::{
    BackendReport, DEFAULT, available_names, build, introspect, readiness_is_usable, select,
    select_as, selected_name,
};
