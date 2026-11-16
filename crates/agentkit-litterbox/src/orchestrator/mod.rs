//! Pairing a branch with compute.
//!
//! `orchestrator/service.rs` is the only module that creates or deletes a
//! branch; a backend is handed a prepared workspace and never learns that a
//! branch exists.

pub mod bulk;
pub mod lock;
pub mod machine;
pub mod outcome;
pub mod service;

pub use bulk::PauseSummary;
pub use outcome::{Deletion, InspectionReport, Performed, Recovery};
pub use service::{
    HostReport, HostRequirement, Listing, PausedPolicy, PlannedStep, Preflight, SandboxIo,
    Sandboxes, expose_store, preflight, preflight_host, resolve_project_slug, store_path,
};
