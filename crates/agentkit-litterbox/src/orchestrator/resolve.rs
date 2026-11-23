//! Resolving a conflict inside the sandbox.
//!
//! Begin opens a resolution and writes the conflicted revision into the
//! workspace, finish asks the backend whether the conflict is gone and performs
//! the host write, and abandon writes the merge again. The revision under
//! resolution is read from the working copy on every operation, never stored,
//! because resolving rewrites it.

use std::path::Path;
use std::sync::Arc;

use crate::metadata_store::{self, StoredResolution, StoredSandbox};
use crate::orchestrator::service::contract_error;
use crate::sandbox::contract::Error;
use crate::sandbox::contract::WorkspaceBinding;
use crate::sandbox::selected::Selected;
use crate::vcs::{Conflict, Revision, Vcs, Workspace};

/// What one resolution action achieved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Resolved {
    /// The revision both sides agreed on when resolution began.
    pub(crate) synced: Revision,
    /// The conflicts the working copy still carries.
    pub(crate) conflicts: Vec<Conflict>,
}

/// The actions one sandbox's resolution can take.
pub(crate) struct Resolver<'a> {
    vcs: &'a Arc<dyn Vcs>,
    stored: &'a StoredSandbox,
    selected: &'a Selected,
    records_dir: &'a Path,
    resolution: std::sync::Mutex<Option<StoredResolution>>,
}

impl<'a> Resolver<'a> {
    /// Build a resolver for one sandbox.
    pub(crate) fn new(
        vcs: &'a Arc<dyn Vcs>,
        stored: &'a StoredSandbox,
        selected: &'a Selected,
        records_dir: &'a Path,
    ) -> Self {
        Self {
            vcs,
            stored,
            selected,
            records_dir,
            resolution: std::sync::Mutex::new(stored.resolution.clone()),
        }
    }

    fn workspace(&self) -> Workspace {
        Workspace::from_branch(&self.stored.branch_name)
    }

    fn open(&self) -> Result<StoredResolution, Error> {
        self.resolution
            .lock()
            .map_err(|_| Error::failed("the resolution lock is poisoned".to_string()))?
            .clone()
            .ok_or_else(|| {
                Error::failed(
                    "no resolution is open for this sandbox; call begin first".to_string(),
                )
            })
    }

    fn record(&self, resolution: Option<StoredResolution>) -> Result<(), Error> {
        let mut updated = self.stored.clone();
        updated.resolution = resolution.clone();
        metadata_store::store_sandbox_at(self.records_dir, &updated.sandbox.name, &updated)?;
        *self
            .resolution
            .lock()
            .map_err(|_| Error::failed("the resolution lock is poisoned".to_string()))? =
            resolution;
        Ok(())
    }

    /// Open a resolution, write the conflict into the workspace, and report
    /// the conflicts it has to settle.
    pub(crate) async fn begin(&self) -> Result<Resolved, Error> {
        if let Some(open) = self.resolution.lock().ok().and_then(|open| open.clone()) {
            return Err(Error::failed(format!(
                "a resolution is already open from {}",
                open.synced
            )));
        }
        let workspace = self.workspace();
        let branch = self.vcs.tip(&workspace).map_err(contract_error)?;
        let conflicts = self.vcs.conflicts(&branch).map_err(contract_error)?;
        if conflicts.is_empty() {
            return Err(Error::failed(
                "the shared branch carries no conflict to resolve".to_string(),
            ));
        }
        let synced = self.vcs.synced(&workspace).map_err(contract_error)?;
        let ours = self.vcs.sandbox_tip(&workspace).map_err(contract_error)?;
        let revision = self.vcs.resolve_begin(&workspace).map_err(contract_error)?;
        if self.stored.binding() == WorkspaceBinding::Copied {
            self.publish(&workspace, &revision).await?;
        }
        let conflicts = self.vcs.conflicts(&revision).map_err(contract_error)?;
        self.record(Some(StoredResolution {
            synced: synced.as_str().to_string(),
            ours: ours.as_str().to_string(),
        }))?;
        Ok(Resolved { synced, conflicts })
    }

    /// Write a revision into a copied sandbox's workspace.
    async fn publish(&self, workspace: &Workspace, revision: &Revision) -> Result<(), Error> {
        let staging = tempfile::tempdir()
            .map_err(|error| Error::failed(format!("could not create a staging dir: {error}")))?;
        self.vcs
            .materialise(workspace, revision, staging.path())
            .map_err(contract_error)?;
        let path = self
            .selected
            .core()
            .inspect(&self.stored.sandbox)
            .await?
            .workspace;
        self.selected
            .core()
            .upload(&self.stored.sandbox, staging.path(), &path)
            .await?;
        Ok(())
    }

    /// Close a resolution, refusing while the working copy still conflicts.
    pub(crate) fn finish(&self) -> Result<Resolved, Error> {
        let open = self.open()?;
        let workspace = self.workspace();
        let revision = self.vcs.sandbox_tip(&workspace).map_err(contract_error)?;
        let conflicts = self.vcs.conflicts(&revision).map_err(contract_error)?;
        if !conflicts.is_empty() {
            return Err(Error::failed(format!(
                "{} conflicted paths remain; resolve them before finishing",
                conflicts.len()
            )));
        }
        self.record(None)?;
        Ok(Resolved {
            synced: Revision::new(open.synced),
            conflicts,
        })
    }

    /// Close a resolution and put the workspace back on the revision it held
    /// before the resolution began.
    pub(crate) async fn abandon(&self) -> Result<Resolved, Error> {
        let open = self.open()?;
        let workspace = self.workspace();
        let ours = Revision::new(open.ours.clone());
        let revision = self
            .vcs
            .resolve_abandon(&workspace, &ours)
            .map_err(contract_error)?;
        if self.stored.binding() == WorkspaceBinding::Copied {
            self.publish(&workspace, &revision).await?;
        }
        self.record(None)?;
        Ok(Resolved {
            synced: Revision::new(open.synced),
            conflicts: Vec::new(),
        })
    }
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod resolution_tests;
