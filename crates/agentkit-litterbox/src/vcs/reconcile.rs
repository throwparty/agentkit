use std::path::PathBuf;

use super::Revision;

/// A host-side view of a diverged sandbox.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Report {
    /// The revision both sides last agreed on.
    pub base: Revision,
    /// The branch tip.
    pub branch: Revision,
    /// The sandbox branch tip, when it is addressable.
    pub sandbox: Option<Revision>,
    /// The paths that would conflict.
    pub conflicts: Vec<PathBuf>,
    /// The staging worktree, when one exists.
    pub worktree: Option<PathBuf>,
    /// The ref whose reflog holds the previous tips.
    pub reflog: Option<String>,
    /// What the user does next, when the sandbox revision is absent.
    pub next: Option<String>,
}

/// A staged merge in a linked worktree.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Staged {
    /// The worktree path.
    pub path: PathBuf,
    /// The branch the worktree holds.
    pub branch: Revision,
    /// The paths that conflict.
    pub conflicts: Vec<PathBuf>,
}

/// The conflict a resolution began.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Begun {
    /// The revision the workspace now holds.
    pub revision: Revision,
}

/// The outcome of finishing a staged merge.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub enum Finished {
    /// The branch was fast-forwarded to the merge commit.
    Landed {
        /// The new branch tip.
        branch: Revision,
    },
    /// The user's checkout holds the branch, so the command to run is printed.
    Deferred {
        /// The command the user runs in their checkout.
        command: String,
    },
}
