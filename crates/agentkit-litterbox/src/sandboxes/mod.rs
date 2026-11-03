//! Pairing a branch with compute.
//!
//! `sandboxes/service.rs` is the only module that creates or deletes a
//! branch; a backend is handed a prepared workspace and never learns that a
//! branch exists.

pub mod bulk;
pub mod service;

pub use service::{Listing, Sandboxes, WORKSPACE_MOUNT, resolve_project_slug};
