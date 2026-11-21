//! A recording VCS double, beside [`crate::testing::TestBackend`].
//!
//! Orchestration tests assert what the orchestrator asked the VCS to do, not
//! what Git did. Git behaviour is tested in `crate::vcs::git`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::sandbox::contract::{BindingSupport, SnapshotSelector};
use crate::vcs::{
    Capabilities, Conflict, ConflictModel, ConflictSides, Error, ExportSupport, HistoryPolicy,
    Merge, Prepared, RestoreMode, Revision, TreeId, Vcs, Workspace, git_common,
};

/// How `prepare` should fail, if at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Failure {
    /// The prepare succeeds.
    #[default]
    None,
    /// The branch already exists.
    Exists,
    /// The branch is gone.
    NotFound,
}

#[derive(Default)]
struct State {
    prepared: Vec<String>,
    materialised: Vec<(String, String)>,
    snapshots: Vec<(String, String)>,
    deleted: Vec<String>,
    exposed: Vec<String>,
    prunes: usize,
    sandboxes: Vec<String>,
    tip: String,
    failure: Failure,
}

impl Default for TestVcs {
    fn default() -> Self {
        Self::new()
    }
}

/// A VCS that records the calls the orchestrator makes.
pub struct TestVcs {
    state: Mutex<State>,
}

impl TestVcs {
    /// Build an empty backend.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State {
                tip: "0123456789abcdef".to_string(),
                ..State::default()
            }),
        }
    }

    /// Configure the names `sandboxes` returns.
    pub fn with_sandboxes(mut self, sandboxes: &[&str]) -> Self {
        self.state_mut().sandboxes = sandboxes.iter().map(|name| name.to_string()).collect();
        self
    }

    /// Make `prepare` fail as if the branch already exists.
    pub fn failing_prepare_exists(mut self) -> Self {
        self.state_mut().failure = Failure::Exists;
        self
    }

    /// Make `prepare` fail as if the branch is gone.
    pub fn failing_prepare_not_found(mut self) -> Self {
        self.state_mut().failure = Failure::NotFound;
        self
    }

    /// The sandbox names passed to `prepare`.
    pub fn prepared(&self) -> Vec<String> {
        self.state().prepared.clone()
    }

    /// The `(branch, revision)` pairs passed to `materialise`.
    pub fn materialised(&self) -> Vec<(String, String)> {
        self.state().materialised.clone()
    }

    /// The `(branch, message)` pairs passed to `snapshot`.
    pub fn snapshots(&self) -> Vec<(String, String)> {
        self.state().snapshots.clone()
    }

    /// The sandbox names passed to `delete_branch`.
    pub fn deleted(&self) -> Vec<String> {
        self.state().deleted.clone()
    }

    /// The remote names passed to `expose`.
    pub fn exposed(&self) -> Vec<String> {
        self.state().exposed.clone()
    }

    /// How many times `prune` was called.
    pub fn prunes(&self) -> usize {
        self.state().prunes
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("TestVcs mutex")
    }

    fn state_mut(&mut self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("TestVcs mutex")
    }
}

struct SelectAll;

impl SnapshotSelector for SelectAll {
    fn selects(
        &self,
        _relative: &Path,
        _is_dir: bool,
    ) -> Result<bool, crate::sandbox::contract::Error> {
        Ok(true)
    }
}

impl Vcs for TestVcs {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            conflict_model: ConflictModel::Blocking,
            op_log: false,
            bindings: BindingSupport::Copied,
            export: ExportSupport::None,
        }
    }

    fn prepare(&self, sandbox: &str) -> Result<Prepared, Error> {
        let mut state = self.state();
        state.prepared.push(sandbox.to_string());
        match state.failure {
            Failure::None => {}
            Failure::Exists => {
                return Err(Error::Exists {
                    name: sandbox.to_string(),
                });
            }
            Failure::NotFound => {
                return Err(Error::NotFound {
                    name: sandbox.to_string(),
                });
            }
        }
        Ok(Prepared {
            workspace: Workspace::from_branch(git_common::branch_name_for(sandbox)),
            base: Revision::new("base"),
        })
    }

    fn masked_paths(&self, _workspace: &Workspace) -> Vec<PathBuf> {
        Vec::new()
    }

    fn identity(&self, _workspace: &Workspace) -> Result<TreeId, Error> {
        Ok(TreeId::new("tree"))
    }

    fn materialise(
        &self,
        workspace: &Workspace,
        revision: &Revision,
        _into: &Path,
    ) -> Result<(), Error> {
        self.state().materialised.push((
            workspace.branch().as_str().to_string(),
            revision.as_str().to_string(),
        ));
        Ok(())
    }

    fn snapshot_filter(&self, _rules_root: &Path) -> Result<Arc<dyn SnapshotSelector>, Error> {
        Ok(Arc::new(SelectAll))
    }

    fn ignore_files(&self) -> Arc<dyn SnapshotSelector> {
        Arc::new(SelectAll)
    }

    fn snapshot(
        &self,
        workspace: &Workspace,
        _staging: &Path,
        message: &str,
    ) -> Result<Revision, Error> {
        self.state()
            .snapshots
            .push((workspace.branch().as_str().to_string(), message.to_string()));
        Ok(Revision::new("snapshot"))
    }

    fn sandboxes(&self) -> Result<Vec<String>, Error> {
        Ok(self.state().sandboxes.clone())
    }

    fn delete_branch(&self, sandbox: &str) -> Result<(), Error> {
        self.state().deleted.push(sandbox.to_string());
        Ok(())
    }

    fn state_path(&self) -> Option<PathBuf> {
        Some(PathBuf::from("/litterbox/test/store"))
    }

    fn expose(&self, name: &str) -> Result<bool, Error> {
        self.state().exposed.push(name.to_string());
        Ok(true)
    }

    fn prune(&self) -> Result<(), Error> {
        self.state().prunes += 1;
        Ok(())
    }

    fn tip(&self, _workspace: &Workspace) -> Result<Revision, Error> {
        Ok(Revision::new(self.state().tip.clone()))
    }

    fn set_tip(&self, _workspace: &Workspace, _revision: &Revision) -> Result<(), Error> {
        Err(unsupported("set_tip"))
    }

    fn synced(&self, _workspace: &Workspace) -> Result<Revision, Error> {
        Err(unsupported("synced"))
    }

    fn set_synced(&self, _workspace: &Workspace, _revision: &Revision) -> Result<(), Error> {
        Err(unsupported("set_synced"))
    }

    fn merge(
        &self,
        _base: &Revision,
        _ours: &Revision,
        _theirs: &Revision,
    ) -> Result<Merge, Error> {
        Err(unsupported("merge"))
    }

    fn conflicts(&self, _revision: &Revision) -> Result<Vec<Conflict>, Error> {
        Err(unsupported("conflicts"))
    }

    fn conflict_sides(&self, _revision: &Revision, _path: &Path) -> Result<ConflictSides, Error> {
        Err(unsupported("conflict_sides"))
    }

    fn resolve(&self, _revision: &Revision, _paths: &[PathBuf]) -> Result<Revision, Error> {
        Err(unsupported("resolve"))
    }

    fn abort(&self, _revision: &Revision) -> Result<(), Error> {
        Err(unsupported("abort"))
    }

    fn history(
        &self,
        _workspace: &Workspace,
        _keep: &HistoryPolicy,
    ) -> Result<Vec<Revision>, Error> {
        Err(unsupported("history"))
    }

    fn restore(
        &self,
        _workspace: &Workspace,
        _revision: &Revision,
        _mode: RestoreMode,
    ) -> Result<(), Error> {
        Err(unsupported("restore"))
    }

    fn export(&self) -> Result<(), Error> {
        Ok(())
    }
}

fn unsupported(operation: &str) -> Error {
    Error::Unsupported {
        operation: operation.to_string(),
        backend: "test".to_string(),
        primitive: "nothing".to_string(),
        reason: "not used by orchestration tests".to_string(),
        remedy: "do not call it".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_records_the_call_and_returns_a_branch() {
        let vcs = TestVcs::new();
        let prepared = vcs.prepare("example").expect("prepare");

        assert_eq!(vcs.prepared(), vec!["example"]);
        assert_eq!(prepared.workspace.branch().as_str(), "litterbox/example");
    }

    #[test]
    fn prepare_can_report_exists_or_not_found() {
        let exists = TestVcs::new().failing_prepare_exists();
        let not_found = TestVcs::new().failing_prepare_not_found();

        assert!(matches!(
            exists.prepare("example"),
            Err(Error::Exists { .. })
        ));
        assert!(matches!(
            not_found.prepare("example"),
            Err(Error::NotFound { .. })
        ));
    }

    #[test]
    fn materialise_and_snapshot_are_recorded() {
        let vcs = TestVcs::new();
        let prepared = vcs.prepare("example").expect("prepare");
        let staging = Path::new("/tmp/staging");

        vcs.materialise(&prepared.workspace, &prepared.base, staging)
            .expect("materialise");
        vcs.snapshot(&prepared.workspace, staging, "message")
            .expect("snapshot");

        assert_eq!(
            vcs.materialised(),
            vec![("litterbox/example".to_string(), "base".to_string())]
        );
        assert_eq!(
            vcs.snapshots(),
            vec![("litterbox/example".to_string(), "message".to_string())]
        );
    }

    #[test]
    fn configured_sandboxes_are_returned() {
        let vcs = TestVcs::new().with_sandboxes(&["one", "two"]);

        assert_eq!(vcs.sandboxes().expect("sandboxes"), vec!["one", "two"]);
    }
}
