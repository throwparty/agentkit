//! Outcomes the orchestrator returns so a surface can warn about a repair.

use std::path::PathBuf;

use crate::sandbox::contract::{Inspection, SandboxStatus};

/// What a repair did, carried alongside the operation's own result.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Recovery {
    /// The sandbox that was repaired.
    pub sandbox: String,
    /// The state the repair started from.
    pub from: SandboxStatus,
    /// What the repair did.
    pub action: String,
    /// The branch the sandbox is paired with.
    pub branch: String,
}

/// One operation's result, plus the repair it performed on the way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Performed<T> {
    /// The operation's own result.
    pub value: T,
    /// The repair, when the operation had to recreate compute.
    pub recovery: Option<Recovery>,
}

impl<T> Performed<T> {
    /// A result that did not repair anything.
    pub fn fresh(value: T) -> Self {
        Self {
            value,
            recovery: None,
        }
    }

    /// A result that repaired the sandbox first.
    pub fn repaired(value: T, recovery: Recovery) -> Self {
        Self {
            value,
            recovery: Some(recovery),
        }
    }

    /// Keep the result and attach a second recovery if this one has none.
    pub fn or_recovery(self, recovery: Option<Recovery>) -> Self {
        if self.recovery.is_some() {
            self
        } else {
            Self {
                value: self.value,
                recovery,
            }
        }
    }
}

/// What enumeration could say about one record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InspectionReport {
    /// The backend was asked and answered.
    Inspected(Inspection),
    /// The backend could not be asked, or answered that the record cannot be
    /// addressed. The status is `Unknown` or `Error` and is never fabricated.
    Status {
        /// The sandbox name.
        name: String,
        /// The backend that could not answer.
        backend: String,
        /// The status-only report.
        status: SandboxStatus,
        /// Why there is no inspection.
        reason: String,
    },
}

impl InspectionReport {
    /// The sandbox name.
    pub fn name(&self) -> &str {
        match self {
            Self::Inspected(inspection) => &inspection.name,
            Self::Status { name, .. } => name,
        }
    }

    /// The status the report carries.
    pub fn status(&self) -> &SandboxStatus {
        match self {
            Self::Inspected(inspection) => &inspection.status,
            Self::Status { status, .. } => status,
        }
    }
}

/// What a delete removed and what it could not stop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deletion {
    /// The metadata file removed, when there was one.
    pub record_path: Option<PathBuf>,
    /// State paths the backend removed.
    pub state_paths: Vec<PathBuf>,
    /// The branch the sandbox was paired with.
    pub branch: String,
    /// Whether the backend knew it stopped the compute.
    pub compute_stopped: bool,
    /// Anything the caller should know, such as compute it could not identify.
    pub detail: Option<String>,
}
