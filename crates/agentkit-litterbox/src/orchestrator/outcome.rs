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
    /// Where the workspace was materialised from: `sandbox` or `shared`.
    pub source: String,
    /// Why the shared branch was used, when the sandbox branch was absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
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
    /// The sandbox branch tip, when it is addressable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    /// The recorded sandbox tip when a resolution began, when one is open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ours: Option<String>,
    /// `clean`, `diverged` or `resolving`.
    pub state: String,
    /// Why the source state is not `clean`, when it is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What the user does next, when the sandbox revision is absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    /// Whether a resolution is open.
    pub resolution: bool,
    /// The conflicts the working copy or shared branch carries, with counts and no text.
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
    /// `clean`, `diverged` or `resolving`.
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
    /// The sandbox tip adoption dropped, when it dropped one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped: Option<String>,
}

/// One operation's result, plus the repair it performed on the way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Performed<T> {
    /// The operation's own result.
    pub value: T,
    /// The repair, when the operation had to recreate compute.
    pub recovery: Option<Recovery>,
    reconciliation: Option<Reconciled>,
    recording_error: Option<String>,
}

impl<T> Performed<T> {
    /// A result that did not repair anything.
    pub fn fresh(value: T) -> Self {
        Self {
            value,
            recovery: None,
            reconciliation: None,
            recording_error: None,
        }
    }

    /// A result that repaired the sandbox first.
    pub fn repaired(value: T, recovery: Recovery) -> Self {
        Self {
            value,
            recovery: Some(recovery),
            reconciliation: None,
            recording_error: None,
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
                recording_error: self.recording_error,
            }
        }
    }

    /// Keep the result and attach the reconciliation the operation performed.
    pub(crate) fn with_reconciliation(mut self, reconciled: Reconciled) -> Self {
        self.reconciliation = Some(reconciled);
        self
    }

    /// Keep the result and report separately why it could not be recorded.
    ///
    /// The operation itself succeeded; only the snapshot that should make it
    /// durable failed. Callers must not treat this as a failed operation.
    pub(crate) fn with_recording_error(mut self, error: String) -> Self {
        self.recording_error = Some(error);
        self
    }

    /// Why the operation's result could not be recorded, when it could not.
    pub fn recording_error(&self) -> Option<&str> {
        self.recording_error.as_deref()
    }

    /// Map the operation's value while preserving the recovery and the
    /// reconciliation the operation reported.
    pub(crate) fn map<U>(self, map: impl FnOnce(T) -> U) -> Performed<U> {
        Performed {
            value: map(self.value),
            recovery: self.recovery,
            reconciliation: self.reconciliation,
            recording_error: self.recording_error,
        }
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

/// Which delete guards the caller has overridden.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeleteGuards {
    /// Permit deleting a sandbox whose compute is running.
    pub running: bool,
    /// Permit deleting a record whose compute cannot be determined.
    pub invalid: bool,
    /// Permit deleting work that may not be merged into the shared branch.
    pub unmerged: bool,
}

impl DeleteGuards {
    /// No guard overridden.
    pub const NONE: Self = Self {
        running: false,
        invalid: false,
        unmerged: false,
    };

    /// Every guard overridden.
    pub const ALL: Self = Self {
        running: true,
        invalid: true,
        unmerged: true,
    };
}

/// What a delete removed and what it could not stop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deletion {
    /// The metadata file removed, when there was one.
    pub record_path: Option<PathBuf>,
    /// State paths the backend removed.
    pub state_paths: Vec<PathBuf>,
    /// The refs the delete removed: shared branch, sandbox branch, synced ref.
    pub branches: Vec<String>,
    /// Whether the backend knew it stopped the compute.
    pub compute_stopped: bool,
    /// The staging workspace removed, when there was one.
    pub staging: Option<PathBuf>,
    /// Guard flags passed for conditions that did not hold.
    pub unused: Vec<String>,
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
            dropped: None,
        }
    }

    #[test]
    fn a_diverged_block_carries_its_reason_and_conflicts() {
        let text = toml::to_string(&block(
            "diverged",
            Some("shared branch is conflicted"),
            &["src/lib.rs"],
            Some("resolve"),
        ))
        .expect("toml");
        assert!(text.contains("state = \"diverged\""), "{text}");
        assert!(text.contains("shared branch is conflicted"), "{text}");
        assert!(text.contains("src/lib.rs"), "{text}");
    }

    #[test]
    fn a_clean_block_omits_reason_and_next() {
        let text = toml::to_string(&block("clean", None, &[], None)).expect("toml");
        assert!(!text.contains("reason"), "{text}");
        assert!(!text.contains("next"), "{text}");
    }

    #[test]
    fn a_dropped_tip_is_carried_in_the_block() {
        let mut block = block("clean", None, &[], None);
        block.dropped = Some("abcd".to_string());
        let text = toml::to_string(&block).expect("toml");
        assert!(text.contains("dropped = \"abcd\""), "{text}");
    }
}
