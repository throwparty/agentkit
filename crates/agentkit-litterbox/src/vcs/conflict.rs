use std::path::PathBuf;

use super::{Revision, TreeId};

/// One conflicted path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// The path, relative to the workspace root.
    pub path: PathBuf,
    /// How many separate conflict regions the path holds.
    pub hunks: usize,
    /// How many content sides the conflict has, excluding any common base.
    pub sides: usize,
    /// Whether the conflict has a common ancestor to merge against.
    ///
    /// An addition on both sides has none.
    pub base: bool,
}

/// The three sides of one conflicted path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictSides {
    /// The common ancestor, when one exists.
    pub base: Option<TreeId>,
    /// The side the branch carries.
    pub ours: TreeId,
    /// The side being merged in.
    pub theirs: TreeId,
}

/// The outcome of a merge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Merge {
    /// The merge produced a revision with no conflicts.
    Clean {
        /// The merged revision.
        revision: Revision,
    },
    /// The merge is held until its conflicts are resolved.
    Blocked {
        /// The paths in conflict.
        conflicts: Vec<Conflict>,
    },
}

/// How much history to retain. User configuration, with no default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryPolicy {
    /// How many steps to keep, when the user configured a limit.
    pub keep: Option<usize>,
}

/// What a restore replaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreMode {
    /// Replace the workspace content.
    WorkingCopy,
    /// Move the branch.
    Branch,
}
