//! The source state and the plan that reconciles it.
//!
//! This module is pure. It reads no file, takes no lock, reads no clock and
//! names no ref. The orchestrator computes the inputs and executes the steps.

use crate::vcs::{ConflictModel, Revision, TreeId};

/// How the shared branch moved relative to the revision both sides agreed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    /// The branch tip is the synced revision.
    Synced,
    /// The tip moved but every file is identical.
    HistoryMoved,
    /// The tip moved and the tree differs.
    ContentMoved,
}

/// Whether the sandbox holds work its own branch does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Workspace {
    /// The workspace tree equals the sandbox branch tree.
    Clean,
    /// The workspace tree differs from the sandbox branch tree.
    Dirty,
}

/// What the record says about a held reconciliation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Merge {
    /// No reconciliation is held.
    #[default]
    None,
    /// The branch moved while the sandbox had unsaved work.
    Pending,
    /// A native backend holds a conflict in a commit.
    Conflicted,
}

/// The three axes at one operation boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceState {
    /// How the shared branch moved.
    pub base: Base,
    /// Whether the sandbox has unsaved work.
    pub workspace: Workspace,
    /// What the record holds.
    pub merge: Merge,
}

/// What the caller is about to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Run a sandbox tool.
    Tool,
    /// Commit the workspace to the sandbox branch.
    Snapshot,
    /// Recreate missing compute.
    Repair,
    /// Resolve a native conflict.
    Resolve,
    /// Abandon a native conflict.
    Abort,
}

/// Why a step holds the branch advance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockReason {
    /// A native conflict has to be resolved before the branch can move.
    Conflict,
    /// Both sides changed and the host has to reconcile them.
    Diverged,
}

/// One ordered step of the plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Do nothing before the operation.
    Proceed,
    /// Advance the synced revision to the branch tip.
    AdoptHistory,
    /// Replace the workspace with the branch tree.
    AdoptContent,
    /// Merge on a backend whose conflicts live in commits.
    Merge,
    /// Commit the workspace on the sandbox branch.
    Snapshot,
    /// Hold the branch advance for the stated reason.
    Block(BlockReason),
    /// Resolve a native conflict.
    Resolve,
    /// Abandon a native conflict.
    Abort,
    /// Recreate the workspace from a revision.
    Materialise,
}

/// Classify the state from the two tips and the four trees.
///
/// Ancestry never decides. The tips settle `Synced` against the other two
/// values; the trees settle everything else.
#[allow(clippy::too_many_arguments)]
pub fn classify(
    synced: &Revision,
    branch: &Revision,
    synced_tree: &TreeId,
    branch_tree: &TreeId,
    sandbox_tree: &TreeId,
    workspace_tree: &TreeId,
    merge: Merge,
) -> SourceState {
    let base = if branch == synced {
        Base::Synced
    } else if branch_tree == synced_tree {
        Base::HistoryMoved
    } else {
        Base::ContentMoved
    };
    let workspace = if workspace_tree == sandbox_tree {
        Workspace::Clean
    } else {
        Workspace::Dirty
    };

    SourceState {
        base,
        workspace,
        merge,
    }
}

/// The ordered steps for one state and action.
///
/// This is the only code that reads the axes. The table is fixed by the spec.
pub fn plan(state: SourceState, action: Action, model: ConflictModel) -> Vec<Step> {
    use Base::{ContentMoved, HistoryMoved, Synced};
    use Step::{AdoptContent, AdoptHistory, Block, Materialise, Proceed, Resolve, Snapshot};
    use Workspace::{Clean, Dirty};

    if state.merge == Merge::Pending {
        return match action {
            Action::Tool => vec![Proceed],
            Action::Snapshot => vec![Snapshot],
            Action::Repair => vec![Materialise],
            Action::Resolve | Action::Abort => vec![Proceed],
        };
    }

    if state.merge == Merge::Conflicted {
        return match (action, model) {
            (Action::Resolve, ConflictModel::Native) => vec![Resolve],
            (Action::Abort, ConflictModel::Native) => vec![Step::Abort],
            (Action::Resolve | Action::Abort, ConflictModel::Blocking) => vec![Proceed],
            (Action::Tool, _) => vec![Proceed],
            (Action::Snapshot, ConflictModel::Native) => vec![Snapshot],
            (Action::Snapshot, ConflictModel::Blocking) => vec![Block(BlockReason::Conflict)],
            (Action::Repair, _) => vec![Materialise],
        };
    }

    if action == Action::Repair {
        return vec![Materialise];
    }

    match (state.base, state.workspace) {
        (Synced, Clean) => vec![Proceed],
        (Synced, Dirty) => vec![Snapshot],
        (HistoryMoved, Clean) => vec![AdoptHistory],
        (HistoryMoved, Dirty) => vec![AdoptHistory, Snapshot],
        (ContentMoved, Clean) => vec![AdoptContent],
        (ContentMoved, Dirty) => vec![Block(BlockReason::Diverged)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::ConflictModel::{Blocking, Native};

    fn state(base: Base, workspace: Workspace, merge: Merge) -> SourceState {
        SourceState {
            base,
            workspace,
            merge,
        }
    }

    #[test]
    fn the_tips_settle_synced_and_history_moved() {
        let first = TreeId::new("tree-one");
        let second = TreeId::new("tree-two");
        let synced = Revision::new("aaaa");
        let moved = Revision::new("bbbb");

        assert_eq!(
            classify(
                &synced,
                &synced,
                &first,
                &first,
                &first,
                &first,
                Merge::None
            )
            .base,
            Base::Synced
        );
        assert_eq!(
            classify(&synced, &moved, &first, &first, &first, &first, Merge::None).base,
            Base::HistoryMoved
        );
        assert_eq!(
            classify(
                &synced,
                &moved,
                &first,
                &second,
                &first,
                &first,
                Merge::None
            )
            .base,
            Base::ContentMoved
        );
    }

    #[test]
    fn the_workspace_is_measured_against_the_sandbox_branch() {
        let keys = TreeId::new("tree-one");
        let edited = TreeId::new("tree-two");
        let synced = Revision::new("aaaa");
        let branch = Revision::new("bbbb");

        assert_eq!(
            classify(&synced, &branch, &keys, &keys, &keys, &keys, Merge::None).workspace,
            Workspace::Clean
        );
        assert_eq!(
            classify(&synced, &branch, &keys, &keys, &keys, &edited, Merge::None).workspace,
            Workspace::Dirty
        );
    }

    #[test]
    fn a_synced_sandbox_proceeds_or_snapshots() {
        assert_eq!(
            plan(
                state(Base::Synced, Workspace::Clean, Merge::None),
                Action::Tool,
                Blocking
            ),
            vec![Step::Proceed]
        );
        assert_eq!(
            plan(
                state(Base::Synced, Workspace::Dirty, Merge::None),
                Action::Tool,
                Blocking
            ),
            vec![Step::Snapshot]
        );
    }

    #[test]
    fn a_moved_history_only_advances_synced() {
        assert_eq!(
            plan(
                state(Base::HistoryMoved, Workspace::Clean, Merge::None),
                Action::Tool,
                Blocking
            ),
            vec![Step::AdoptHistory]
        );
        assert_eq!(
            plan(
                state(Base::HistoryMoved, Workspace::Dirty, Merge::None),
                Action::Snapshot,
                Blocking
            ),
            vec![Step::AdoptHistory, Step::Snapshot]
        );
    }

    #[test]
    fn moved_content_is_adopted_only_when_the_workspace_is_clean() {
        assert_eq!(
            plan(
                state(Base::ContentMoved, Workspace::Clean, Merge::None),
                Action::Tool,
                Blocking
            ),
            vec![Step::AdoptContent]
        );
        assert_eq!(
            plan(
                state(Base::ContentMoved, Workspace::Dirty, Merge::None),
                Action::Tool,
                Blocking
            ),
            vec![Step::Block(BlockReason::Diverged)]
        );
    }

    #[test]
    fn a_pending_branch_holds_the_advance_and_keeps_snapshotting() {
        assert_eq!(
            plan(
                state(Base::ContentMoved, Workspace::Dirty, Merge::Pending),
                Action::Tool,
                Blocking
            ),
            vec![Step::Proceed]
        );
        assert_eq!(
            plan(
                state(Base::ContentMoved, Workspace::Dirty, Merge::Pending),
                Action::Snapshot,
                Blocking
            ),
            vec![Step::Snapshot]
        );
        assert_eq!(
            plan(
                state(Base::ContentMoved, Workspace::Clean, Merge::Pending),
                Action::Repair,
                Blocking
            ),
            vec![Step::Materialise]
        );
    }

    #[test]
    fn a_native_conflict_resolves_and_a_blocking_one_holds_the_snapshot() {
        let conflicted = state(Base::ContentMoved, Workspace::Dirty, Merge::Conflicted);

        assert_eq!(plan(conflicted, Action::Tool, Native), vec![Step::Proceed]);
        assert_eq!(
            plan(conflicted, Action::Snapshot, Native),
            vec![Step::Snapshot]
        );
        assert_eq!(
            plan(conflicted, Action::Resolve, Native),
            vec![Step::Resolve]
        );
        assert_eq!(plan(conflicted, Action::Abort, Native), vec![Step::Abort]);

        assert_eq!(
            plan(conflicted, Action::Snapshot, Blocking),
            vec![Step::Block(BlockReason::Conflict)]
        );
        assert_eq!(
            plan(conflicted, Action::Resolve, Blocking),
            vec![Step::Proceed]
        );
        assert_eq!(
            plan(conflicted, Action::Abort, Blocking),
            vec![Step::Proceed]
        );
    }

    #[test]
    fn a_repair_materialises_in_every_state_without_a_held_merge() {
        for base in [Base::Synced, Base::HistoryMoved, Base::ContentMoved] {
            for workspace in [Workspace::Clean, Workspace::Dirty] {
                assert_eq!(
                    plan(
                        state(base, workspace, Merge::None),
                        Action::Repair,
                        Blocking
                    ),
                    vec![Step::Materialise]
                );
            }
        }
    }

    #[test]
    fn git_never_returns_merge_resolve_or_abort() {
        for merge in [Merge::None, Merge::Pending, Merge::Conflicted] {
            for base in [Base::Synced, Base::HistoryMoved, Base::ContentMoved] {
                for workspace in [Workspace::Clean, Workspace::Dirty] {
                    for action in [
                        Action::Tool,
                        Action::Snapshot,
                        Action::Repair,
                        Action::Resolve,
                        Action::Abort,
                    ] {
                        let steps = plan(state(base, workspace, merge), action, Blocking);
                        assert!(
                            !steps.iter().any(|step| matches!(
                                step,
                                Step::Merge | Step::Resolve | Step::Abort
                            )),
                            "{merge:?} {base:?} {workspace:?} {action:?} returned {steps:?}"
                        );
                    }
                }
            }
        }
    }
}
