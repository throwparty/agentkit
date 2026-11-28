use crate::vcs::HistoryStep;

/// The operator's view of one sandbox's history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryReport {
    /// The sandbox.
    pub sandbox: String,
    /// The steps, newest first.
    pub steps: Vec<HistoryStep>,
    /// `clean`, `diverged` or `resolving`.
    pub state: String,
    /// The sandbox branch commit, when it is readable.
    pub sandbox_tip: Option<String>,
    /// The next command, when the sandbox needs one.
    pub next: Option<String>,
}

/// The result of a restore.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRestore {
    /// The sandbox.
    pub sandbox: String,
    /// The commit the operator selected.
    pub target: String,
    /// The commit the shared branch now points at.
    pub revision: String,
    /// Whether a new commit was created.
    pub created: bool,
    /// What happened to the sandbox workspace: `replaced`, `updated by the
    /// VCS`, or `deferred; repair will materialise the restored revision`.
    pub workspace: String,
}

/// What a restore may discard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RestoreFlags {
    /// Discard a held merge, unsaved work or a staging workspace.
    pub discard_work: bool,
    /// Discard an open resolution.
    pub discard_resolution: bool,
}
