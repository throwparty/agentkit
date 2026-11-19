//! The VCS contract.
//!
//! The orchestrator names this module and nothing below it. Revisions and trees
//! are opaque: a backend decides what they contain, and no backend type appears
//! in a signature here. Implementations live in [`crate::vcs::git`] and, later,
//! `crate::vcs::jj`.

mod capabilities;
mod conflict;
mod error;
pub mod git;
pub mod git_common;
mod types;

pub use capabilities::{BindingSupport, Capabilities, ConflictModel, ExportSupport};
pub use conflict::{Conflict, ConflictSides, HistoryPolicy, Merge, RestoreMode};
pub use error::Error;
pub use types::{BranchName, Prepared, Revision, TreeId, Workspace};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::sandbox::contract::SnapshotSelector;

/// One VCS implementation behind the orchestrator's handle.
///
/// Object-safe and synchronous. The orchestrator stores `Arc<dyn Vcs>` and
/// calls these methods from async code, exactly as it called the Git-specific
/// `Scm` trait before.
pub trait Vcs: Send + Sync {
    /// What this backend and its configuration can do.
    fn capabilities(&self) -> Capabilities;

    /// Create or attach the branch for `sandbox` and report the revision it
    /// starts from. Used by create.
    fn prepare(&self, sandbox: &str) -> Result<Prepared, Error>;

    /// Paths the sandbox must not see inside a bound workspace.
    fn masked_paths(&self, workspace: &Workspace) -> Vec<PathBuf>;

    /// The tree identity of the workspace's branch tip.
    fn identity(&self, workspace: &Workspace) -> Result<TreeId, Error>;

    /// Unpack `revision` into `into` as the workspace source.
    fn materialise(
        &self,
        workspace: &Workspace,
        revision: &Revision,
        into: &Path,
    ) -> Result<(), Error>;

    /// The transfer predicate for a snapshot, read from `rules_root`.
    fn snapshot_filter(&self, rules_root: &Path) -> Result<Arc<dyn SnapshotSelector>, Error>;

    /// Commit the staged tree into the workspace's branch.
    fn snapshot(
        &self,
        workspace: &Workspace,
        staging: &Path,
        message: &str,
    ) -> Result<Revision, Error>;

    /// The sandbox names this backend exposes in its configured locality.
    fn sandboxes(&self) -> Result<Vec<String>, Error>;

    /// Delete the branch paired with `sandbox`.
    fn delete_branch(&self, sandbox: &str) -> Result<(), Error>;

    /// Where this backend keeps durable project state, if it has a path.
    fn state_path(&self) -> Option<PathBuf>;

    /// Attach the state at [`Self::state_path`] to the user's checkout as a
    /// remote called `name`. Returns whether it was added.
    fn expose(&self, name: &str) -> Result<bool, Error>;

    /// Release project storage that is no longer needed.
    fn prune(&self) -> Result<(), Error>;

    /// The revision at the workspace branch tip.
    fn tip(&self, workspace: &Workspace) -> Result<Revision, Error>;

    /// Move the workspace branch tip.
    fn set_tip(&self, workspace: &Workspace, revision: &Revision) -> Result<(), Error>;

    /// The revision the branch was last synchronised at.
    fn synced(&self, workspace: &Workspace) -> Result<Revision, Error>;

    /// Record the revision the branch was synchronised at.
    fn set_synced(&self, workspace: &Workspace, revision: &Revision) -> Result<(), Error>;

    /// Merge three revisions according to this backend's conflict model.
    fn merge(&self, base: &Revision, ours: &Revision, theirs: &Revision) -> Result<Merge, Error>;

    /// The conflicts a revision carries.
    fn conflicts(&self, revision: &Revision) -> Result<Vec<Conflict>, Error>;

    /// The three sides of one conflicted path.
    fn conflict_sides(&self, revision: &Revision, path: &Path) -> Result<ConflictSides, Error>;

    /// Mark the given paths resolved in `revision`.
    fn resolve(&self, revision: &Revision, paths: &[PathBuf]) -> Result<Revision, Error>;

    /// Abandon a merge.
    fn abort(&self, revision: &Revision) -> Result<(), Error>;

    /// The retained revisions for a branch under `keep`.
    fn history(&self, workspace: &Workspace, keep: &HistoryPolicy) -> Result<Vec<Revision>, Error>;

    /// Restore a branch to a revision.
    fn restore(
        &self,
        workspace: &Workspace,
        revision: &Revision,
        mode: RestoreMode,
    ) -> Result<(), Error>;

    /// Publish the backend's state to its external form, if it has one.
    fn export(&self) -> Result<(), Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Stub;

    impl Vcs for Stub {
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                conflict_model: ConflictModel::Blocking,
                op_log: false,
                bindings: BindingSupport::Copied,
                export: ExportSupport::None,
            }
        }

        fn prepare(&self, _sandbox: &str) -> Result<Prepared, Error> {
            unsupported("prepare")
        }

        fn masked_paths(&self, _workspace: &Workspace) -> Vec<PathBuf> {
            Vec::new()
        }

        fn identity(&self, _workspace: &Workspace) -> Result<TreeId, Error> {
            unsupported("identity")
        }

        fn materialise(
            &self,
            _workspace: &Workspace,
            _revision: &Revision,
            _into: &Path,
        ) -> Result<(), Error> {
            unsupported("materialise")
        }

        fn snapshot_filter(&self, _rules_root: &Path) -> Result<Arc<dyn SnapshotSelector>, Error> {
            unsupported("snapshot_filter")
        }

        fn snapshot(
            &self,
            _workspace: &Workspace,
            _staging: &Path,
            _message: &str,
        ) -> Result<Revision, Error> {
            unsupported("snapshot")
        }

        fn sandboxes(&self) -> Result<Vec<String>, Error> {
            Ok(Vec::new())
        }

        fn delete_branch(&self, _sandbox: &str) -> Result<(), Error> {
            unsupported("delete_branch")
        }

        fn state_path(&self) -> Option<PathBuf> {
            None
        }

        fn expose(&self, _name: &str) -> Result<bool, Error> {
            unsupported("expose")
        }

        fn prune(&self) -> Result<(), Error> {
            Ok(())
        }

        fn tip(&self, _workspace: &Workspace) -> Result<Revision, Error> {
            unsupported("tip")
        }

        fn set_tip(&self, _workspace: &Workspace, _revision: &Revision) -> Result<(), Error> {
            unsupported("set_tip")
        }

        fn synced(&self, _workspace: &Workspace) -> Result<Revision, Error> {
            unsupported("synced")
        }

        fn set_synced(&self, _workspace: &Workspace, _revision: &Revision) -> Result<(), Error> {
            unsupported("set_synced")
        }

        fn merge(
            &self,
            _base: &Revision,
            _ours: &Revision,
            _theirs: &Revision,
        ) -> Result<Merge, Error> {
            unsupported("merge")
        }

        fn conflicts(&self, _revision: &Revision) -> Result<Vec<Conflict>, Error> {
            unsupported("conflicts")
        }

        fn conflict_sides(
            &self,
            _revision: &Revision,
            _path: &Path,
        ) -> Result<ConflictSides, Error> {
            unsupported("conflict_sides")
        }

        fn resolve(&self, _revision: &Revision, _paths: &[PathBuf]) -> Result<Revision, Error> {
            unsupported("resolve")
        }

        fn abort(&self, _revision: &Revision) -> Result<(), Error> {
            unsupported("abort")
        }

        fn history(
            &self,
            _workspace: &Workspace,
            _keep: &HistoryPolicy,
        ) -> Result<Vec<Revision>, Error> {
            unsupported("history")
        }

        fn restore(
            &self,
            _workspace: &Workspace,
            _revision: &Revision,
            _mode: RestoreMode,
        ) -> Result<(), Error> {
            unsupported("restore")
        }

        fn export(&self) -> Result<(), Error> {
            Ok(())
        }
    }

    fn unsupported<T>(operation: &str) -> Result<T, Error> {
        Err(Error::Unsupported {
            operation: operation.to_string(),
            backend: "stub".to_string(),
            primitive: "nothing".to_string(),
            reason: "stub".to_string(),
            remedy: "use a real backend".to_string(),
        })
    }

    #[test]
    fn the_trait_is_object_safe_and_shared() {
        let vcs: Arc<dyn Vcs> = Arc::new(Stub);
        let handle = Arc::clone(&vcs);
        let thread = std::thread::spawn(move || handle.capabilities().conflict_model);

        assert_eq!(vcs.capabilities().bindings, BindingSupport::Copied);
        assert_eq!(thread.join().expect("joined"), ConflictModel::Blocking);
    }

    #[test]
    fn workspace_addresses_a_branch_without_a_backend() {
        let workspace = Workspace::from_branch("litterbox/example");

        assert_eq!(workspace.branch().as_str(), "litterbox/example");
        assert_eq!(BranchName::new("main").as_str(), "main");
    }

    #[test]
    fn a_backend_error_keeps_its_message() {
        let error = Error::Backend {
            operation: "open".to_string(),
            message: "Git repository open failed: no repository".to_string(),
        };

        assert_eq!(
            error.to_string(),
            "Git repository open failed: no repository"
        );
    }
}
