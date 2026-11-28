//! Resolving a conflict inside the sandbox.
//!
//! Begin opens a resolution and writes the conflicted revision into the
//! workspace, finish asks the backend whether the conflict is gone and performs
//! the host write, and abandon writes the merge again. The revision under
//! resolution is read from the working copy on every operation, never stored,
//! because resolving rewrites it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::metadata_store::{self, StoredResolution, StoredSandbox};
use crate::orchestrator::reconcile::Reconciler;
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
    /// Paths the sandbox side had that the resolution dropped.
    pub(crate) dropped_from_ours: Vec<PathBuf>,
    /// Paths the shared branch had that the resolution dropped.
    pub(crate) dropped_from_theirs: Vec<PathBuf>,
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

    /// Refuse to resolve onto a shared tip that is already broken.
    fn reject_conflicted_branch(&self, branch: &Revision) -> Result<(), Error> {
        let conflicted = self.vcs.conflicts(branch).map_err(contract_error)?;
        if conflicted.is_empty() {
            return Ok(());
        }
        // The broken commit belongs to the user, and only they can replace
        // it. Litterbox reports it and holds until they do.
        let fix = match self.stored.vcs() {
            "jj" => "for example `jj edit <revision>`, resolve, then `jj bookmark set <branch> -r @`",
            _ => "for example `git commit --amend` or a new commit, then push",
        };
        Err(Error::Denied {
            operation: "resolve the shared branch".to_string(),
            reason: format!(
                "the shared branch tip already carries a conflict in {} path(s). Litterbox \
                 resolves only the merge it creates; fix the tip in your checkout and push \
                 it, {}",
                conflicted.len(),
                fix
            ),
        })
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
        self.reject_conflicted_branch(&branch)?;

        // Record the workspace before reading the sandbox side. An edit that
        // has not been snapshotted yet is part of what the agent is resolving;
        // publishing the merge over it would drop it from `ours`.
        let reconciler = Reconciler::new(self.vcs, self.stored, self.selected, self.records_dir);
        reconciler
            .snapshot("litterbox: snapshot before resolution")
            .await?;

        let branch = self.vcs.tip(&workspace).map_err(contract_error)?;
        self.reject_conflicted_branch(&branch)?;
        let synced = self.vcs.synced(&workspace).map_err(contract_error)?;
        let ours = self.vcs.sandbox_tip(&workspace).map_err(contract_error)?;
        let begun = self.vcs.resolve_begin(&workspace).map_err(contract_error)?;
        let conflicts = self
            .vcs
            .conflicts(&begun.revision)
            .map_err(contract_error)?;
        if conflicts.is_empty() {
            // Nothing conflicted, so there is nothing to resolve: put the
            // workspace back the way it was rather than leaving a merge behind.
            let restored = self
                .vcs
                .resolve_abandon(&workspace, &ours)
                .map_err(contract_error)?;
            if self.stored.binding() == WorkspaceBinding::Copied {
                self.publish(&ours, &restored).await?;
            }
            return Err(Error::failed(
                "the branches merge without a conflict; there is nothing to resolve".to_string(),
            ));
        }
        if self.stored.binding() == WorkspaceBinding::Copied {
            self.publish(&ours, &begun.revision).await?;
        }
        self.record(Some(StoredResolution {
            synced: synced.as_str().to_string(),
            ours: ours.as_str().to_string(),
            conflict: begun.revision.as_str().to_string(),
            theirs: branch.as_str().to_string(),
        }))?;
        Ok(Resolved {
            synced,
            conflicts,
            dropped_from_ours: Vec::new(),
            dropped_from_theirs: Vec::new(),
        })
    }

    /// Write a revision into a copied sandbox's workspace.
    async fn publish(&self, old: &Revision, revision: &Revision) -> Result<(), Error> {
        let reconciler = Reconciler::new(self.vcs, self.stored, self.selected, self.records_dir);
        reconciler.replace_workspace_with(revision, old).await
    }

    /// Close a resolution, failing while the working copy still conflicts.
    ///
    /// Nothing blocks the write on the report. A union legitimately drops a
    /// side, so the report names what is gone and the VCS verdict decides.
    pub(crate) fn finish(&self) -> Result<Resolved, Error> {
        let open = self.open()?;
        let workspace = self.workspace();
        let revision = self
            .vcs
            .working_copy_tip(&workspace)
            .map_err(contract_error)?;
        let conflicts = self.vcs.conflicts(&revision).map_err(contract_error)?;
        if !conflicts.is_empty() {
            return Err(Error::failed(format!(
                "{} conflicted paths remain; resolve them before finishing",
                conflicts.len()
            )));
        }

        let theirs = if open.theirs.is_empty() {
            self.vcs.tip(&workspace).map_err(contract_error)?
        } else {
            Revision::new(open.theirs.clone())
        };
        let resolved = self.vcs.tree(&revision).map_err(contract_error)?;
        let ours_revision = Revision::new(open.ours.clone());
        let ours = self.vcs.tree(&ours_revision).map_err(contract_error)?;
        let theirs_tree = self.vcs.tree(&theirs).map_err(contract_error)?;
        let dropped_from_ours = self
            .vcs
            .deleted_paths(&ours, &resolved)
            .map_err(contract_error)?;
        let dropped_from_theirs = self
            .vcs
            .deleted_paths(&theirs_tree, &resolved)
            .map_err(contract_error)?;

        match self.vcs.advance(&workspace, &revision, &theirs) {
            Ok(()) => {}
            Err(crate::vcs::Error::Raced) => {
                return Err(Error::failed(
                    "the shared branch moved while the resolution was open; the resolution was \
                     not written. Re-run begin against the new tip."
                        .to_string(),
                ));
            }
            Err(error) => return Err(contract_error(error)),
        }
        // The working copy is now published. Record it as the saved baseline
        // and leave a fresh working copy, so later edits cannot rewrite it.
        self.vcs
            .set_sandbox_tip(&workspace, &revision)
            .map_err(contract_error)?;
        let landed = self.vcs.tip(&workspace).map_err(contract_error)?;
        self.vcs
            .set_synced(&workspace, &landed)
            .map_err(contract_error)?;
        self.record(None)?;
        Ok(Resolved {
            synced: Revision::new(open.synced),
            conflicts,
            dropped_from_ours,
            dropped_from_theirs,
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
            // The sandbox holds the conflict tree unless a later snapshot
            // moved the saved baseline. Deletions are relative to the content
            // the sandbox actually has, not to `ours`.
            let saved = self.vcs.sandbox_tip(&workspace).map_err(contract_error)?;
            let old = if saved == ours && !open.conflict.is_empty() {
                Revision::new(open.conflict.clone())
            } else {
                saved
            };
            self.publish(&old, &revision).await?;
        }
        self.record(None)?;
        Ok(Resolved {
            synced: Revision::new(open.synced),
            conflicts: Vec::new(),
            dropped_from_ours: Vec::new(),
            dropped_from_theirs: Vec::new(),
        })
    }
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod resolution_tests;
