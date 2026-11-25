use super::Revision;

/// One commit the shared branch pointed at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryStep {
    /// The commit the branch pointed at.
    pub revision: Revision,
    /// When the branch moved to it, in Unix seconds.
    pub time: i64,
    /// The log's description of the move.
    pub description: String,
    /// Whether this commit was the synced revision.
    pub synced: bool,
}

/// The result of a restore.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restored {
    /// The commit the shared branch now points at.
    pub revision: Revision,
    /// Whether a new commit was created.
    pub created: bool,
}
