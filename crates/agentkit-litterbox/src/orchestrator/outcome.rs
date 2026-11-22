//! Outcomes the orchestrator returns so a surface can warn about a repair.

use std::path::PathBuf;

use crate::orchestrator::reconcile::Reconciled;
use crate::orchestrator::source::SourceState;
use crate::sandbox::contract::{Inspection, SandboxStatus};
use crate::vcs::Revision;

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
    reconciliation: Option<Reconciled>,
}

impl<T> Performed<T> {
    /// A result that did not repair anything.
    pub fn fresh(value: T) -> Self {
        Self {
            value,
            recovery: None,
            reconciliation: None,
        }
    }

    /// A result that repaired the sandbox first.
    pub fn repaired(value: T, recovery: Recovery) -> Self {
        Self {
            value,
            recovery: Some(recovery),
            reconciliation: None,
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
                reconciliation: self.reconciliation,
            }
        }
    }

    /// Attach the reconciliation the operation performed.
    pub(crate) fn with_reconciliation(mut self, reconciled: Reconciled) -> Self {
        self.reconciliation = Some(reconciled);
        self
    }

    /// The source state after the reconciliation, when one ran.
    pub fn state(&self) -> Option<&SourceState> {
        self.reconciliation.as_ref().map(|done| &done.state)
    }

    /// The sandbox tip adoption dropped, when it dropped one.
    pub fn dropped(&self) -> Option<&Revision> {
        self.reconciliation
            .as_ref()
            .and_then(|done| done.dropped.as_ref())
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

/// What finishing a staged merge did.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub enum StagedMerge {
    /// The branch was fast-forwarded to the merge commit.
    Landed {
        /// The new branch tip.
        branch: String,
    },
    /// The user's checkout holds the branch, so the command to run is printed.
    Deferred {
        /// The command the user runs in their checkout.
        command: String,
    },
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
