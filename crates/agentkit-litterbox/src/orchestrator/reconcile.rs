//! The reconciliation adapter.
//!
//! It reads the state through the contract, asks [`crate::orchestrator::source`]
//! for the steps, and executes them. It names no ref and no backend.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tempfile::TempDir;

use crate::metadata_store::{self, StoredMerge, StoredSandbox};
use crate::orchestrator::outcome::{ConflictView, ResolutionView, WorkspaceBlock};
use crate::orchestrator::service::contract_error;
use crate::orchestrator::source::{self, Action, Merge, SourceState, Step};
use crate::sandbox::contract::{Error, SnapshotSelector, WorkspaceBinding};
use crate::sandbox::selected::Selected;
use crate::vcs::{Revision, TreeId, Vcs, Workspace};

/// How many times a race with the user's own push is retried.
const MAX_ATTEMPTS: usize = 3;

/// What one plan execution achieved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Reconciled {
    /// The state after the plan ran.
    pub(crate) state: SourceState,
    /// The sandbox tip adoption dropped, when it dropped one.
    pub(crate) dropped: Option<Revision>,
    /// The model-facing block for this operation.
    pub(crate) block: WorkspaceBlock,
}

/// The directory a snapshot reads, and the temporary one that may own it.
struct WorkspaceSource {
    path: PathBuf,
    _temp: Option<TempDir>,
}

/// The facts one classification read, kept for the steps that follow.
struct Observation {
    synced: Revision,
    branch: Revision,
    sandbox: Revision,
    source: Option<WorkspaceSource>,
    block: WorkspaceBlock,
}

/// One reconciliation attempt that lost a race and can be planned again.
struct Raced;

/// The adapter between the orchestrator and the plan.
pub(crate) struct Reconciler<'a> {
    vcs: &'a Arc<dyn Vcs>,
    stored: &'a StoredSandbox,
    selected: &'a Selected,
    records_dir: &'a Path,
    merge: Mutex<Option<StoredMerge>>,
}

impl<'a> Reconciler<'a> {
    /// Build a reconciler for one operation.
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
            merge: Mutex::new(stored.merge.clone()),
        }
    }

    /// Plan and run the steps that come before a tool.
    pub(crate) async fn before(&self, action: Action) -> Result<Reconciled, Error> {
        self.run(action, None).await
    }

    /// Plan and run the snapshot that comes after a tool.
    pub(crate) async fn after(&self, message: &str) -> Result<Reconciled, Error> {
        self.run(Action::Snapshot, Some(message)).await
    }

    /// Plan and run an explicit snapshot.
    pub(crate) async fn snapshot(&self, message: &str) -> Result<Reconciled, Error> {
        self.run(Action::Snapshot, Some(message)).await
    }

    async fn run(&self, action: Action, message: Option<&str>) -> Result<Reconciled, Error> {
        let message = message.unwrap_or("litterbox: snapshot");
        for _ in 0..MAX_ATTEMPTS {
            match self.attempt(action, message).await? {
                Ok(done) => return Ok(done),
                Err(Raced) => continue,
            }
        }
        Err(Error::failed(
            "the branch moved while the sandbox was reconciling; try again".to_string(),
        ))
    }

    async fn attempt(
        &self,
        action: Action,
        message: &str,
    ) -> Result<Result<Reconciled, Raced>, Error> {
        let (state, observed) = self.classify_with(action, false, true).await?;
        let steps = source::plan(state, action, self.vcs.capabilities().conflict_model);

        let mut block = observed.block.clone();
        match self.execute(state, &steps, &observed, message).await? {
            Ok((dropped, state)) => {
                if let Some(dropped) = &dropped {
                    block.dropped = Some(dropped.as_str().to_string());
                }
                Ok(Ok(Reconciled {
                    state,
                    dropped,
                    block,
                }))
            }
            Err(Raced) => Ok(Err(Raced)),
        }
    }

    /// Read the state without changing anything.
    ///
    /// The workspace is always observed so a restore can tell whether it would
    /// lose unsaved work. A held merge is not cleared: a refused restore must
    /// write nothing.
    pub(crate) async fn state(&self) -> Result<SourceState, Error> {
        self.classify_with(Action::Tool, true, false)
            .await
            .map(|(state, _)| state)
    }

    /// Read the state, and the workspace tree when the plan can use it.
    async fn classify_with(
        &self,
        action: Action,
        force_observation: bool,
        clear_merge: bool,
    ) -> Result<(SourceState, Observation), Error> {
        let workspace = self.workspace_handle();
        self.vcs.ensure_refs(&workspace).map_err(contract_error)?;

        let synced = self.vcs.synced(&workspace).map_err(contract_error)?;
        let branch = self.vcs.tip(&workspace).map_err(contract_error)?;
        let sandbox = self.vcs.sandbox_tip(&workspace).map_err(contract_error)?;
        let synced_tree = self.vcs.tree(&synced).map_err(contract_error)?;
        let branch_tree = self.vcs.tree(&branch).map_err(contract_error)?;
        let sandbox_tree = self.vcs.tree(&sandbox).map_err(contract_error)?;

        // A conflicted shared branch is held: nothing is adopted, and a
        // snapshot stays on the sandbox branch. A backend with no conflict
        // check reports none, which leaves its behaviour unchanged.
        let conflicted: Vec<std::path::PathBuf> = match self.vcs.conflicts(&branch) {
            Ok(conflicts) => conflicts
                .into_iter()
                .map(|conflict| conflict.path)
                .collect(),
            Err(crate::vcs::Error::Unsupported { .. }) => Vec::new(),
            Err(error) => return Err(contract_error(error)),
        };

        let mut merge = merge_state(
            self.merge
                .lock()
                .map_err(|_| Error::failed("the merge state lock is poisoned".to_string()))?
                .as_ref(),
        );
        if clear_merge
            && merge == Merge::Pending
            && self
                .vcs
                .is_ancestor(&sandbox, &branch)
                .map_err(contract_error)?
        {
            self.clear_merge()?;
            merge = Merge::None;
        }

        if !conflicted.is_empty() {
            merge = Merge::Pending;
        }

        // An open resolution is held like a pending merge: tools run and
        // snapshot, but only `finish` advances the shared branch. Without
        // this, a clean shared branch would let the next tool adopt content
        // over the conflict tree.
        let resolving = self.stored.resolution.is_some();
        if resolving {
            merge = Merge::Resolving;
        }

        // The pre-plan reads the workspace only when the result can change the
        // steps: a moved branch, a held merge, or a snapshot.
        let needs_observation =
            force_observation || needs_observation(merge, &branch, &synced, action);
        let (workspace_tree, observed_source) = if needs_observation {
            let (source, tree) = self.observe().await?;
            (tree, Some(source))
        } else {
            // The previous operation's snapshot left the sandbox at its own tip.
            (sandbox_tree.clone(), None)
        };

        let state = source::classify(
            &synced,
            &branch,
            &synced_tree,
            &branch_tree,
            &sandbox_tree,
            &workspace_tree,
            merge,
        );

        let resolution_conflicts = if resolving {
            let working = self
                .vcs
                .working_copy_tip(&workspace)
                .map_err(contract_error)?;
            let base = self
                .stored
                .resolution
                .as_ref()
                .map(|open| Revision::new(open.synced.clone()));
            self.vcs
                .conflicts_with_base(&working, base.as_ref())
                .map_err(contract_error)?
        } else {
            Vec::new()
        };
        let held = !conflicted.is_empty() || merge == Merge::Pending;
        let block = WorkspaceBlock {
            state: if resolving {
                "resolving"
            } else if held {
                "diverged"
            } else {
                "clean"
            }
            .to_string(),
            reason: if resolving {
                None
            } else if !conflicted.is_empty() {
                Some("shared branch is conflicted".to_string())
            } else if merge == Merge::Pending {
                Some("the sandbox has unsaved work and the branch moved".to_string())
            } else {
                None
            },
            synced: synced.as_str().to_string(),
            branch: branch.as_str().to_string(),
            sandbox: sandbox.as_str().to_string(),
            conflicts: if resolving {
                resolution_conflicts
                    .iter()
                    .map(|conflict| conflict.path.display().to_string())
                    .collect()
            } else {
                conflicted
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect()
            },
            next: if resolving {
                Some("resolve the conflicts in the workspace, then finish or abandon".to_string())
            } else if !conflicted.is_empty() {
                Some(
                    "resolve the shared branch and push; the sandbox is on the last good revision"
                        .to_string(),
                )
            } else if merge == Merge::Pending {
                Some("run `litterbox reconcile`".to_string())
            } else {
                None
            },
            resolution: self.stored.resolution.as_ref().map(|open| ResolutionView {
                synced: open.synced.clone(),
                conflicts: resolution_conflicts
                    .iter()
                    .map(|conflict| ConflictView {
                        path: conflict.path.display().to_string(),
                        hunks: conflict.hunks,
                        sides: conflict.sides,
                        base: conflict.base,
                    })
                    .collect(),
                next: "resolve the conflicts in the workspace, then finish or abandon".to_string(),
            }),
            dropped: None,
        };

        Ok((
            state,
            Observation {
                synced,
                branch,
                sandbox,
                source: observed_source,
                block,
            },
        ))
    }

    async fn execute(
        &self,
        state: SourceState,
        steps: &[Step],
        observed: &Observation,
        message: &str,
    ) -> Result<Result<(Option<Revision>, SourceState), Raced>, Error> {
        let workspace = self.workspace_handle();
        let mut dropped = None;
        let mut post = state;

        for step in steps {
            match step {
                Step::Proceed => {}
                Step::AdoptHistory => {
                    // The branch history moved without a content change. Only
                    // move the sandbox ref when it is already contained in the
                    // adopted history; otherwise sandbox-only history would be
                    // dropped, and a later snapshot would reintroduce the old
                    // parent over the rewritten branch.
                    if self
                        .vcs
                        .is_ancestor(&observed.sandbox, &observed.branch)
                        .map_err(contract_error)?
                    {
                        self.vcs
                            .set_sandbox_tip(&workspace, &observed.branch)
                            .map_err(contract_error)?;
                        self.vcs
                            .set_synced(&workspace, &observed.branch)
                            .map_err(contract_error)?;
                        post.base = source::Base::Synced;
                        post.merge = Merge::None;
                    } else {
                        self.record_merge(observed)?;
                        post.merge = Merge::Pending;
                    }
                }
                Step::AdoptContent => {
                    if !self
                        .vcs
                        .is_ancestor(&observed.sandbox, &observed.branch)
                        .map_err(contract_error)?
                    {
                        dropped = Some(observed.sandbox.clone());
                    }
                    self.adopt(&observed.branch, &observed.sandbox).await?;
                    self.vcs
                        .set_sandbox_tip(&workspace, &observed.branch)
                        .map_err(contract_error)?;
                    self.vcs
                        .set_synced(&workspace, &observed.branch)
                        .map_err(contract_error)?;
                    post.base = source::Base::Synced;
                    post.merge = Merge::None;
                }
                Step::Snapshot => {
                    let Some(source) = &observed.source else {
                        return Err(Error::failed(
                            "the plan asked for a snapshot without reading the workspace"
                                .to_string(),
                        ));
                    };
                    let revision = self
                        .vcs
                        .snapshot(&workspace, &source.path, message)
                        .map_err(contract_error)?;
                    if state.merge != Merge::Pending && state.merge != Merge::Resolving {
                        match self.vcs.advance(&workspace, &revision, &observed.branch) {
                            Ok(()) => {
                                self.vcs
                                    .set_synced(&workspace, &revision)
                                    .map_err(contract_error)?;
                                post.base = source::Base::Synced;
                                post.merge = Merge::None;
                                post.workspace = source::Workspace::Clean;
                            }
                            Err(crate::vcs::Error::Raced) => {
                                // The snapshot is already on the sandbox ref. Record it
                                // as a held divergence before retrying, or the retry
                                // sees a clean workspace and adopts over it.
                                self.record_merge_with(&revision, observed)?;
                                return Ok(Err(Raced));
                            }
                            Err(error) => return Err(contract_error(error)),
                        }
                    }
                }
                Step::Block(_) => {
                    self.record_merge(observed)?;
                    post.merge = Merge::Pending;
                }
                Step::Merge | Step::Resolve | Step::Abort | Step::Materialise => {
                    return Err(Error::failed(format!(
                        "the sandbox cannot perform {step:?}"
                    )));
                }
            }
        }

        Ok(Ok((dropped, post)))
    }

    /// Read the workspace and return the tree a snapshot would commit.
    async fn observe(&self) -> Result<(WorkspaceSource, TreeId), Error> {
        let workspace = self.workspace_handle();
        let masked = self.vcs.masked_paths(&workspace);

        // A bound workspace is the host directory, so reading it in place is
        // both correct and cheaper. A copied one has to be downloaded.
        if self.stored.binding() == WorkspaceBinding::Bound {
            let root = self.vcs.bound_root(&workspace).map_err(contract_error)?;
            let tree = self
                .vcs
                .observe(&workspace, &root)
                .map_err(contract_error)?;
            return Ok((
                WorkspaceSource {
                    path: root,
                    _temp: None,
                },
                tree,
            ));
        }

        let path = self.workspace_path().await?;
        let staging = tempfile::tempdir()
            .map_err(|error| Error::failed(format!("could not create a staging dir: {error}")))?;

        let ignore: Arc<dyn SnapshotSelector> = Arc::new(Excluding {
            inner: self.vcs.ignore_files(),
            masked: masked.clone(),
        });
        self.selected
            .core()
            .download(&self.stored.sandbox, &path, staging.path(), Some(ignore))
            .await?;
        let filter = self
            .vcs
            .snapshot_filter(&workspace, staging.path())
            .map_err(contract_error)?;
        let filter: Arc<dyn SnapshotSelector> = Arc::new(Excluding {
            inner: filter,
            masked,
        });
        self.selected
            .core()
            .download(&self.stored.sandbox, &path, staging.path(), Some(filter))
            .await?;

        let tree = self
            .vcs
            .observe(&workspace, staging.path())
            .map_err(contract_error)?;
        Ok((
            WorkspaceSource {
                path: staging.path().to_path_buf(),
                _temp: Some(staging),
            },
            tree,
        ))
    }

    /// Replace the recorded workspace content with the branch tree.
    async fn adopt(&self, branch: &Revision, sandbox: &Revision) -> Result<(), Error> {
        self.replace_workspace_with(branch, sandbox).await?;
        let workspace = self.workspace_handle();
        self.vcs
            .set_sandbox_tip(&workspace, branch)
            .map_err(contract_error)?;
        self.vcs
            .set_synced(&workspace, branch)
            .map_err(contract_error)?;
        Ok(())
    }

    /// Replace the workspace with the target tree, without moving a ref.
    pub(crate) async fn replace_workspace(&self, target: &Revision) -> Result<(), Error> {
        let workspace = self.workspace_handle();
        let sandbox = self.vcs.sandbox_tip(&workspace).map_err(contract_error)?;
        self.replace_workspace_with(target, &sandbox).await
    }

    pub(crate) async fn replace_workspace_with(
        &self,
        target: &Revision,
        old: &Revision,
    ) -> Result<(), Error> {
        let workspace = self.workspace_handle();
        let old_tree = self.vcs.tree(old).map_err(contract_error)?;
        let new_tree = self.vcs.tree(target).map_err(contract_error)?;

        let staging = tempfile::tempdir()
            .map_err(|error| Error::failed(format!("could not create a staging dir: {error}")))?;
        self.vcs
            .materialise(&workspace, target, staging.path())
            .map_err(contract_error)?;

        // `upload` refuses source symlinks. The tree came from the VCS, not
        // from the sandbox, so take the links out, upload the regular entries,
        // and recreate the links in the sandbox explicitly.
        let symlinks = take_symlinks(staging.path())
            .map_err(|error| Error::failed(format!("could not read staged links: {error}")))?;

        let path = self.workspace_path().await?;

        // Remove obsolete tracked paths before uploading. A file replaced by a
        // directory, or the reverse, would otherwise collide with the old
        // entry. Empty directories left behind are removed too; a directory
        // holding ignored files is left alone because rmdir fails.
        let deleted = self
            .vcs
            .deleted_paths(&old_tree, &new_tree)
            .map_err(contract_error)?;
        if !deleted.is_empty() {
            let mut command = vec!["rm".to_string(), "-rf".to_string(), "--".to_string()];
            let mut directories: Vec<PathBuf> = Vec::new();
            for relative in &deleted {
                command.push(format!("{path}/{}", relative.display()));
                let mut parent = relative.parent();
                while let Some(directory) = parent {
                    if directory.as_os_str().is_empty() {
                        break;
                    }
                    if !directories.iter().any(|seen| seen == directory) {
                        directories.push(directory.to_path_buf());
                    }
                    parent = directory.parent();
                }
            }
            let execution = self
                .selected
                .core()
                .exec(&self.stored.sandbox, &command)
                .await?;
            if execution.exit_code != 0 {
                return Err(Error::failed(format!(
                    "removing obsolete paths failed: {}",
                    execution.stderr.trim()
                )));
            }

            // Deepest first, so a parent can become empty after its child.
            directories.sort_by_key(|directory| std::cmp::Reverse(directory.components().count()));
            for directory in directories {
                let command = vec![
                    "rmdir".to_string(),
                    "--".to_string(),
                    format!("{path}/{}", directory.display()),
                ];
                let _ = self
                    .selected
                    .core()
                    .exec(&self.stored.sandbox, &command)
                    .await;
            }
        }

        self.selected
            .core()
            .upload(&self.stored.sandbox, staging.path(), &path)
            .await?;

        for (relative, target) in symlinks {
            let command = vec![
                "ln".to_string(),
                "-sfn".to_string(),
                target.to_string_lossy().into_owned(),
                format!("{path}/{}", relative.display()),
            ];
            let execution = self
                .selected
                .core()
                .exec(&self.stored.sandbox, &command)
                .await?;
            if execution.exit_code != 0 {
                return Err(Error::failed(format!(
                    "restoring the symlink {} failed: {}",
                    relative.display(),
                    execution.stderr.trim()
                )));
            }
        }

        Ok(())
    }

    fn record_merge(&self, observed: &Observation) -> Result<(), Error> {
        self.store_merge(&observed.synced, &observed.sandbox, &observed.branch)
    }

    /// Record a merge whose sandbox side is newer than the observation's.
    fn record_merge_with(&self, ours: &Revision, observed: &Observation) -> Result<(), Error> {
        self.store_merge(&observed.synced, ours, &observed.branch)
    }

    fn store_merge(
        &self,
        base: &Revision,
        ours: &Revision,
        theirs: &Revision,
    ) -> Result<(), Error> {
        let merge = StoredMerge {
            base: base.to_string(),
            ours: ours.to_string(),
            theirs: theirs.to_string(),
        };
        let mut updated = self.stored.clone();
        updated.merge = Some(merge.clone());
        metadata_store::store_sandbox_at(self.records_dir, &updated.sandbox.name, &updated)?;
        *self
            .merge
            .lock()
            .map_err(|_| Error::failed("the merge state lock is poisoned".to_string()))? =
            Some(merge);
        Ok(())
    }

    fn clear_merge(&self) -> Result<(), Error> {
        if self
            .merge
            .lock()
            .map_err(|_| Error::failed("the merge state lock is poisoned".to_string()))?
            .is_none()
        {
            return Ok(());
        }
        let mut updated = self.stored.clone();
        updated.merge = None;
        metadata_store::store_sandbox_at(self.records_dir, &updated.sandbox.name, &updated)?;
        *self
            .merge
            .lock()
            .map_err(|_| Error::failed("the merge state lock is poisoned".to_string()))? = None;
        Ok(())
    }

    fn workspace_handle(&self) -> Workspace {
        Workspace::from_branch(&self.stored.branch_name)
    }

    async fn workspace_path(&self) -> Result<String, Error> {
        Ok(self
            .selected
            .core()
            .inspect(&self.stored.sandbox)
            .await?
            .workspace)
    }
}

/// Remove every symlink under `root`, returning each link's relative path and
/// target. The walk never follows a link.
fn take_symlinks(root: &Path) -> std::io::Result<Vec<(PathBuf, PathBuf)>> {
    fn walk(
        root: &Path,
        current: &Path,
        found: &mut Vec<(PathBuf, PathBuf)>,
    ) -> std::io::Result<()> {
        for entry in std::fs::read_dir(current)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| path.clone());
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                found.push((relative, std::fs::read_link(&path)?));
            } else if metadata.is_dir() {
                walk(root, &path, found)?;
            }
        }
        Ok(())
    }

    let mut found = Vec::new();
    walk(root, root, &mut found)?;
    for (relative, _) in &found {
        std::fs::remove_file(root.join(relative))?;
    }
    Ok(found)
}

fn merge_state(merge: Option<&StoredMerge>) -> Merge {    match merge {
        Some(_) => Merge::Pending,
        None => Merge::None,
    }
}

fn needs_observation(merge: Merge, branch: &Revision, synced: &Revision, action: Action) -> bool {
    merge != Merge::None || branch != synced || action == Action::Snapshot
}

/// A selector that rejects paths the sandbox must not see.
struct Excluding {
    inner: Arc<dyn SnapshotSelector>,
    masked: Vec<PathBuf>,
}

impl SnapshotSelector for Excluding {
    fn selects(&self, relative: &Path, is_dir: bool) -> Result<bool, Error> {
        if self.masked.iter().any(|path| relative.starts_with(path)) {
            return Ok(false);
        }
        self.inner.selects(relative, is_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_synced_read_defers_the_observation() {
        let same = Revision::new("aaaa");
        assert!(!needs_observation(Merge::None, &same, &same, Action::Tool));
        assert!(needs_observation(
            Merge::None,
            &same,
            &same,
            Action::Snapshot
        ));
    }

    #[test]
    fn a_moved_branch_or_a_held_merge_observes() {
        let synced = Revision::new("aaaa");
        let moved = Revision::new("bbbb");
        assert!(needs_observation(
            Merge::None,
            &moved,
            &synced,
            Action::Tool
        ));
        assert!(needs_observation(
            Merge::Pending,
            &synced,
            &synced,
            Action::Tool
        ));
    }

    #[test]
    fn a_stored_merge_reads_as_pending() {
        let held = StoredMerge {
            base: "aaaa".to_string(),
            ours: "bbbb".to_string(),
            theirs: "cccc".to_string(),
        };
        assert_eq!(merge_state(Some(&held)), Merge::Pending);
        assert_eq!(merge_state(None), Merge::None);
    }
}
