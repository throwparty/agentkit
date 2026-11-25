//! The VCS contract.
//!
//! The orchestrator names this module and nothing below it. Revisions and trees
//! are opaque: a backend decides what they contain, and no backend type appears
//! in a signature here. Implementations live in [`crate::vcs::git`] and, later,
//! `crate::vcs::jj`.

mod capabilities;
mod conflict;
mod contract;
mod error;
pub mod git;
pub mod git_common;
mod history;
pub mod jj;
mod reconcile;
mod types;

pub use capabilities::{Capabilities, ConflictModel, ExportSupport};
pub use conflict::{Conflict, ConflictSides, Merge};
pub use contract::Vcs;
pub use error::Error;
pub use history::{HistoryStep, Restored};
pub use reconcile::{Begun, Finished, Report, Staged};
pub use types::{BranchName, Prepared, Revision, TreeId, Workspace};
