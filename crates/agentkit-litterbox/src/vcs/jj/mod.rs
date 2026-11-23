//! The jj backend.
//!
//! It shells out to the `jj` binary. The clone is a co-located jj repository
//! at `vcs/jj/<slug>`; each sandbox is one jj workspace. It imports `vcs` and
//! `vcs::git_common` and never `vcs::git`.

mod config;

pub use config::JjVcsConfig;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::domain::SandboxError;
use crate::sandbox::contract::{BindingSupport, SnapshotSelector};
use crate::vcs::{
    Capabilities, Conflict, ConflictModel, ConflictSides, Error, ExportSupport, HistoryPolicy,
    Merge, Prepared, RestoreMode, Revision, TreeId, Vcs, Workspace,
};

/// A jj-backed VCS.
pub struct JjVcs {
    root: PathBuf,
    #[allow(dead_code)]
    slug: String,
    #[allow(dead_code)]
    host: PathBuf,
    #[allow(dead_code)]
    lock: Mutex<()>,
}

impl JjVcs {
    /// Build the backend for one project.
    pub fn open(project_slug: &str) -> Result<Self, SandboxError> {
        let host = std::env::current_dir().map_err(SandboxError::Io)?;
        Ok(Self {
            root: path_for(project_slug),
            slug: project_slug.to_string(),
            host,
            lock: Mutex::new(()),
        })
    }
}

fn path_for(slug: &str) -> PathBuf {
    agentkit_common::data_dir("litterbox")
        .join("vcs")
        .join("jj")
        .join(slug)
}

fn sandbox_of(workspace: &Workspace) -> &str {
    workspace
        .branch()
        .as_str()
        .strip_prefix("litterbox/")
        .unwrap_or_else(|| workspace.branch().as_str())
}

fn unsupported(operation: &str) -> Error {
    Error::Unsupported {
        operation: operation.to_string(),
        backend: "jj".to_string(),
        primitive: "the jj backend".to_string(),
        reason: "it is not implemented in this change".to_string(),
        remedy: "use the git backend for now".to_string(),
    }
}

struct Everything;

impl SnapshotSelector for Everything {
    fn selects(
        &self,
        _relative: &Path,
        _is_dir: bool,
    ) -> Result<bool, crate::sandbox::contract::Error> {
        Ok(true)
    }
}

impl Vcs for JjVcs {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            conflict_model: ConflictModel::Blocking,
            op_log: true,
            bindings: BindingSupport::Both,
            export: ExportSupport::GitExport,
        }
    }

    fn prepare(&self, _sandbox: &str) -> Result<Prepared, Error> {
        Err(unsupported("prepare"))
    }

    fn masked_paths(&self, _workspace: &Workspace) -> Vec<PathBuf> {
        vec![PathBuf::from(".jj")]
    }

    fn bound_root(&self, workspace: &Workspace) -> Result<PathBuf, Error> {
        Ok(self.root.join("workspaces").join(sandbox_of(workspace)))
    }

    fn identity(&self, _workspace: &Workspace) -> Result<TreeId, Error> {
        Err(unsupported("identity"))
    }

    fn materialise(
        &self,
        _workspace: &Workspace,
        _revision: &Revision,
        _into: &Path,
    ) -> Result<(), Error> {
        Err(unsupported("materialise"))
    }

    fn snapshot_filter(
        &self,
        _rules_root: &Path,
    ) -> Result<std::sync::Arc<dyn SnapshotSelector>, Error> {
        Ok(std::sync::Arc::new(Everything))
    }

    fn ignore_files(&self) -> std::sync::Arc<dyn SnapshotSelector> {
        std::sync::Arc::new(Everything)
    }

    fn snapshot(
        &self,
        _workspace: &Workspace,
        _staging: &Path,
        _message: &str,
    ) -> Result<Revision, Error> {
        Err(unsupported("snapshot"))
    }

    fn sandboxes(&self) -> Result<Vec<String>, Error> {
        Ok(Vec::new())
    }

    fn delete_branch(&self, _sandbox: &str) -> Result<(), Error> {
        Err(unsupported("delete_branch"))
    }

    fn state_path(&self) -> Option<PathBuf> {
        Some(self.root.clone())
    }

    fn expose(&self, _name: &str) -> Result<bool, Error> {
        Err(unsupported("expose"))
    }

    fn prune(&self) -> Result<(), Error> {
        Ok(())
    }

    fn tip(&self, _workspace: &Workspace) -> Result<Revision, Error> {
        Err(unsupported("tip"))
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
