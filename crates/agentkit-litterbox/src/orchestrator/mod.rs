//! Pairing a branch with compute.
//!
//! `orchestrator/service.rs` is the only module that creates or deletes a
//! branch; a backend is handed a prepared workspace and never learns that a
//! branch exists.

pub mod bulk;
pub mod machine;
pub mod service;

pub use bulk::PauseSummary;
pub use service::{
    Listing, PlannedStep, Preflight, SandboxIo, Sandboxes, preflight, resolve_project_slug,
};
