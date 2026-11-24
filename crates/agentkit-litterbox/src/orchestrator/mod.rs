//! Pairing a branch with compute.
//!
//! `orchestrator/service.rs` is the only module that creates or deletes a
//! branch; a backend is handed a prepared workspace and never learns that a
//! branch exists.

pub mod bulk;
pub mod lock;
pub mod machine;
pub mod outcome;
pub mod reconcile;
pub(crate) mod resolve;
pub mod service;
pub mod source;

pub use bulk::PauseSummary;
pub use outcome::{DeleteGuards, Deletion, InspectionReport, Performed, Recovery, StagedMerge};
pub use service::{
    HostReport, HostRequirement, Listing, PausedPolicy, PlannedStep, Preflight, SandboxIo,
    Sandboxes, preflight, preflight_host, resolve_project_slug,
};
