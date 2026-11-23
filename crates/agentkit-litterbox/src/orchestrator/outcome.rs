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

/// One conflicted path, as the model sees it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ConflictView {
    /// The path, relative to the workspace root.
    pub path: String,
    /// How many separate conflict regions the path holds.
    pub hunks: usize,
    /// How many content sides the conflict has.
    pub sides: usize,
    /// Whether the conflict has a common ancestor.
    pub base: bool,
}

/// The source state a status call reports.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SourceStatus {
    /// The revision both sides last agreed on.
    pub synced: String,
    /// The shared branch tip.
    pub branch: String,
    /// The sandbox branch tip.
    pub sandbox: String,
    /// `clean` or `conflicted`.
    pub state: String,
    /// Whether a resolution is open.
    pub resolution: bool,
    /// The conflicts the shared branch carries, with counts and no text.
    pub conflicts: Vec<ConflictView>,
}

/// The resolution an operation observed, when one is open.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ResolutionView {
    /// The revision both sides agreed on when resolution began.
    pub synced: String,
    /// The conflicted paths the working copy still carries.
    pub conflicts: Vec<String>,
    /// What the agent does next.
    pub next: String,
}

/// The model-facing workspace block appended to a single-sandbox tool result.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct WorkspaceBlock {
    /// `clean` or `held`.
    pub state: String,
    /// Why the branch is held, when it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The revision both sides last agreed on.
    pub synced: String,
    /// The shared branch tip.
    pub branch: String,
    /// The sandbox branch tip.
    pub sandbox: String,
    /// The conflicted paths, when there are any.
    pub conflicts: Vec<String>,
    /// The next step the user takes, when one is needed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    /// The resolution the operation observed, when one is open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<ResolutionView>,
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

    /// The workspace block the operation should report, when a plan ran.
    pub fn block(&self) -> Option<WorkspaceBlock> {
        self.reconciliation.as_ref().map(|done| done.block.clone())
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

#[cfg(test)]
mod tests {
    use super::WorkspaceBlock;

    fn block(
        state: &str,
        reason: Option<&str>,
        conflicts: &[&str],
        next: Option<&str>,
    ) -> WorkspaceBlock {
        WorkspaceBlock {
            state: state.to_string(),
            reason: reason.map(str::to_string),
            synced: "s".to_string(),
            branch: "b".to_string(),
            sandbox: "x".to_string(),
            conflicts: conflicts.iter().map(|path| path.to_string()).collect(),
            next: next.map(str::to_string),
            resolution: None,
        }
    }

    #[test]
    fn a_held_block_carries_its_reason_and_conflicts() {
        let text = toml::to_string(&block(
            "held",
            Some("shared branch is conflicted"),
            &["src/lib.rs"],
            Some("resolve"),
        ))
        .expect("toml");
        assert!(text.contains("state = \"held\""), "{text}");
        assert!(text.contains("shared branch is conflicted"), "{text}");
        assert!(text.contains("src/lib.rs"), "{text}");
    }

    #[test]
    fn a_clean_block_omits_reason_and_next() {
        let text = toml::to_string(&block("clean", None, &[], None)).expect("toml");
        assert!(!text.contains("reason"), "{text}");
        assert!(!text.contains("next"), "{text}");
    }
}
