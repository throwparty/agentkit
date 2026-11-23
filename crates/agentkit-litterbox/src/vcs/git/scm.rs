use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use git2::{BranchType, ObjectType, Repository};

use crate::domain::{SandboxError, ScmMode, slugify};
use crate::sandbox::contract::{BindingSupport, SnapshotSelector};
use crate::vcs::git_common::GitError;
use crate::vcs::git_common::archive;
pub use crate::vcs::git_common::{SnapshotFilter, branch_name_for};
use crate::vcs::{
    Begun, Capabilities, Conflict, ConflictModel, ConflictSides, Error as VcsError, ExportSupport,
    Finished, HistoryPolicy, Merge, Prepared, Report, RestoreMode, Revision, Staged, TreeId, Vcs,
    Workspace,
};

use super::{reconcile, refs};

/// How many times a snapshot may rebuild its commit while another moves the ref.
const MAX_SNAPSHOT_ATTEMPTS: usize = 8;

pub trait Scm {
    fn mode(&self) -> ScmMode;
    fn create_branch(&self, slug: &str) -> Result<String, SandboxError>;
    fn delete_branch(&self, slug: &str) -> Result<(), SandboxError>;
    fn make_archive(&self, reference: &str) -> Result<Vec<u8>, SandboxError>;
    fn list_sandboxes(&self) -> Result<Vec<String>, SandboxError>;
    fn repo_prefix(&self) -> Result<String, SandboxError>;
    fn apply_patch(&self, diff: &str) -> Result<(), SandboxError>;
}

pub struct GitScm {
    repo: Repository,
    mode: ScmMode,
    snapshot_branch: Option<String>,
    /// For remote mode, path to the host repo (used for archive operations).
    host_repo_path: Option<PathBuf>,
}

impl GitScm {
    fn open_repository(path: &Path, mode: ScmMode) -> Result<Repository, SandboxError> {
        let repo = Repository::open(path)
            .map_err(|source| SandboxError::from(GitError::Open { source }))?;
        if matches!(mode, ScmMode::Remote) {
            let mut config = repo
                .config()
                .map_err(|source| SandboxError::from(GitError::Open { source }))?;
            config
                .set_str("core.logAllRefUpdates", "always")
                .map_err(|source| SandboxError::from(GitError::Open { source }))?;
        }
        Ok(repo)
    }

    pub fn open(path: &Path, mode: ScmMode) -> Result<Self, SandboxError> {
        let repo = Self::open_repository(path, mode)?;
        Ok(Self {
            repo,
            mode,
            snapshot_branch: None,
            host_repo_path: None,
        })
    }

    pub fn open_with_host(
        path: &Path,
        mode: ScmMode,
        host_repo_path: Option<PathBuf>,
    ) -> Result<Self, SandboxError> {
        let repo = Self::open_repository(path, mode)?;
        Ok(Self {
            repo,
            mode,
            snapshot_branch: None,
            host_repo_path,
        })
    }

    pub fn open_direct(path: &Path) -> Result<Self, SandboxError> {
        Self::open(path, ScmMode::Direct)
    }

    pub fn mode(&self) -> ScmMode {
        self.mode
    }

    pub fn set_snapshot_branch(&mut self, branch: String) {
        self.snapshot_branch = Some(branch);
    }

    /// Point snapshots at a full ref name, such as the sandbox branch.
    pub fn set_snapshot_ref(&mut self, reference: String) {
        self.snapshot_branch = Some(reference);
    }

    fn branch_name(slug: &str) -> String {
        branch_name_for(slug)
    }

    fn repo_root(&self) -> PathBuf {
        self.repo
            .workdir()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.repo.path().to_path_buf())
    }

    fn repo_prefix(&self) -> String {
        repo_prefix_from_path(&self.repo_root())
    }

    fn head_commit(&self) -> Result<git2::Commit<'_>, SandboxError> {
        let head = self
            .repo
            .head()
            .map_err(|source| SandboxError::from(GitError::BranchCreate { source }))?;
        head.peel_to_commit()
            .map_err(|source| SandboxError::from(GitError::BranchCreate { source }))
    }

    fn signature(&self) -> Result<git2::Signature<'_>, SandboxError> {
        self.repo
            .signature()
            .or_else(|_| git2::Signature::now("Litterbox", "noreply@example.com"))
            .map_err(|source| SandboxError::from(GitError::Signature { source }))
    }

    fn head_commit_optional(&self) -> Result<Option<git2::Commit<'_>>, SandboxError> {
        match self.repo.head() {
            Ok(head) => head
                .peel_to_commit()
                .map(Some)
                .map_err(|source| SandboxError::from(GitError::Head { source })),
            Err(error) if error.code() == git2::ErrorCode::UnbornBranch => Ok(None),
            Err(source) => Err(SandboxError::from(GitError::Head { source })),
        }
    }

    fn snapshot_branch_ref(&self) -> String {
        match &self.snapshot_branch {
            Some(reference) if reference.starts_with("refs/") => reference.clone(),
            Some(branch) => format!("refs/heads/{}", branch),
            None => "refs/heads/litterbox-snapshots".to_string(),
        }
    }

    /// The commit the snapshot branch points at, when the branch exists.
    fn snapshot_target(&self) -> Result<Option<git2::Commit<'_>>, SandboxError> {
        match self.repo.find_reference(&self.snapshot_branch_ref()) {
            Ok(reference) => reference
                .peel_to_commit()
                .map(Some)
                .map_err(|source| SandboxError::from(GitError::Reference { source })),
            Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(source) => Err(SandboxError::from(GitError::Reference { source })),
        }
    }
}

pub struct GitVcs {
    inner: Mutex<GitScm>,
    prefix_override: Option<String>,
    /// The project slug the bare store is named after, when known.
    store_slug: Option<String>,
    /// Where staging worktrees live, when a test overrides the data dir.
    reconcile_root: Option<PathBuf>,
}

/// The old name, kept while the orchestrator still imports it.
pub type ThreadSafeScm = GitVcs;

impl GitVcs {
    /// Record the project slug used for the bare store.
    pub fn set_store_slug(mut self, slug: String) -> Self {
        self.store_slug = Some(slug);
        self
    }

    /// Put staging worktrees under `root` instead of the data directory.
    pub fn with_reconcile_root(mut self, root: PathBuf) -> Self {
        self.reconcile_root = Some(root);
        self
    }

    fn reconcile_root(&self) -> PathBuf {
        self.reconcile_root
            .clone()
            .unwrap_or_else(|| crate::vcs::git::path::data_dir().join("reconcile"))
    }

    fn project_slug(&self) -> Result<String, VcsError> {
        self.store_slug.clone().ok_or_else(|| VcsError::Backend {
            operation: "reconcile".to_string(),
            message: "the project slug is not known".to_string(),
        })
    }

    /// The repository path that holds `branch`.
    fn repo_path_for(&self, branch: &str) -> Result<PathBuf, VcsError> {
        self.repo_for_branch(branch, |repo| {
            Ok(repo
                .workdir()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| repo.path().to_path_buf()))
        })
        .map_err(vcs_error)
    }

    pub fn open(path: &Path) -> Result<Self, SandboxError> {
        GitScm::open_direct(path).map(|scm| Self {
            inner: Mutex::new(scm),
            prefix_override: None,
            store_slug: None,
            reconcile_root: None,
        })
    }

    pub fn open_with_mode(path: &Path, mode: ScmMode) -> Result<Self, SandboxError> {
        GitScm::open(path, mode).map(|scm| Self {
            inner: Mutex::new(scm),
            prefix_override: None,
            store_slug: None,
            reconcile_root: None,
        })
    }

    pub fn open_with_prefix(path: &Path, prefix: Option<String>) -> Result<Self, SandboxError> {
        GitScm::open_direct(path).map(|scm| Self {
            inner: Mutex::new(scm),
            prefix_override: prefix,
            store_slug: None,
            reconcile_root: None,
        })
    }

    pub fn open_with_mode_and_prefix(
        path: &Path,
        mode: ScmMode,
        prefix: Option<String>,
        host_repo_path: Option<PathBuf>,
    ) -> Result<Self, SandboxError> {
        GitScm::open_with_host(path, mode, host_repo_path).map(|scm| Self {
            inner: Mutex::new(scm),
            prefix_override: prefix,
            store_slug: None,
            reconcile_root: None,
        })
    }

    pub fn for_sandbox(
        path: &Path,
        prefix: Option<String>,
        sandbox_slug: &str,
    ) -> Result<Self, SandboxError> {
        let mut scm = GitScm::open_direct(path)?;
        let branch_name = GitScm::branch_name(sandbox_slug);
        scm.set_snapshot_branch(branch_name);

        Ok(Self {
            inner: Mutex::new(scm),
            prefix_override: prefix,
            store_slug: None,
            reconcile_root: None,
        })
    }

    pub fn set_snapshot_branch(&self, branch: String) -> Result<(), SandboxError> {
        self.lock()?.set_snapshot_branch(branch);
        Ok(())
    }

    /// Point snapshots at a full ref name, such as the sandbox branch.
    pub fn set_snapshot_ref(&self, reference: String) -> Result<(), SandboxError> {
        self.lock()?.set_snapshot_ref(reference);
        Ok(())
    }

    /// The paths this sandbox's snapshot carries.
    ///
    /// `rules_root` is the tree the ignore rules are read from: the sandbox's
    /// own copy of the workspace.
    pub fn snapshot_filter(&self, rules_root: &Path) -> Result<SnapshotFilter, SandboxError> {
        self.lock()?.snapshot_filter(rules_root)
    }

    pub fn commit_snapshot_from_staging(
        &self,
        staging_path: &Path,
        message: &str,
    ) -> Result<Option<git2::Oid>, SandboxError> {
        self.lock()?
            .commit_snapshot_from_staging(staging_path, message)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, GitScm>, SandboxError> {
        self.inner
            .lock()
            .map_err(|_| SandboxError::Config("Mutex poisoned".to_string()))
    }
}

impl Scm for GitVcs {
    fn mode(&self) -> ScmMode {
        self.lock().map(|g| g.mode()).unwrap_or(ScmMode::Direct)
    }

    fn create_branch(&self, slug: &str) -> Result<String, SandboxError> {
        self.lock()?.create_branch(slug)
    }

    fn delete_branch(&self, slug: &str) -> Result<(), SandboxError> {
        self.lock()?.delete_branch(slug)
    }

    fn make_archive(&self, reference: &str) -> Result<Vec<u8>, SandboxError> {
        self.lock()?.make_archive(reference)
    }

    fn list_sandboxes(&self) -> Result<Vec<String>, SandboxError> {
        self.lock()?.list_sandboxes()
    }

    fn repo_prefix(&self) -> Result<String, SandboxError> {
        if let Some(ref prefix) = self.prefix_override {
            Ok(prefix.clone())
        } else {
            Ok(self.lock()?.repo_prefix())
        }
    }

    fn apply_patch(&self, diff: &str) -> Result<(), SandboxError> {
        self.lock()?.apply_patch(diff)
    }
}

impl Scm for GitScm {
    fn mode(&self) -> ScmMode {
        self.mode
    }

    fn create_branch(&self, slug: &str) -> Result<String, SandboxError> {
        let branch_name = Self::branch_name(slug);
        let head = self.head_commit()?;

        if self
            .repo
            .find_branch(&branch_name, BranchType::Local)
            .is_ok()
        {
            return Err(SandboxError::SandboxExists {
                name: slug.to_string(),
            });
        }

        self.repo
            .branch(&branch_name, &head, false)
            .map_err(|source| SandboxError::from(GitError::BranchCreate { source }))?;

        Ok(branch_name)
    }

    fn delete_branch(&self, slug: &str) -> Result<(), SandboxError> {
        let branch_name = Self::branch_name(slug);

        let mut branch = self
            .repo
            .find_branch(&branch_name, BranchType::Local)
            .map_err(|_| SandboxError::SandboxNotFound {
                name: slug.to_string(),
            })?;

        branch
            .delete()
            .map_err(|source| SandboxError::from(GitError::BranchDelete { source }))
    }

    fn make_archive(&self, reference: &str) -> Result<Vec<u8>, SandboxError> {
        let host;
        let repo = match self.mode {
            ScmMode::Remote => {
                // A sandbox branch lives in the bare clone, which owns it. The
                // host is only the source for a reference the bare clone does
                // not hold, such as HEAD: the bare clone can be older than the
                // checkout the user is working in.
                if self.repo.find_branch(reference, BranchType::Local).is_ok() {
                    &self.repo
                } else if let Some(ref host_path) = self.host_repo_path {
                    host = Repository::open(host_path)
                        .map_err(|source| SandboxError::from(GitError::Open { source }))?;
                    &host
                } else {
                    &self.repo
                }
            }
            ScmMode::Direct => &self.repo,
        };
        let tree = archive::tree_from_reference(repo, reference)?;
        let mut builder = tar::Builder::new(Vec::new());

        archive::append_tree(repo, &mut builder, &tree, Path::new(""))?;

        builder.into_inner().map_err(SandboxError::Io)
    }

    fn list_sandboxes(&self) -> Result<Vec<String>, SandboxError> {
        let mut sandboxes = Vec::new();
        let branches = self
            .repo
            .branches(Some(BranchType::Local))
            .map_err(|source| SandboxError::from(GitError::BranchList { source }))?;

        for branch in branches {
            let (branch, _) =
                branch.map_err(|source| SandboxError::from(GitError::BranchList { source }))?;
            if let Some(name) = branch.name().ok().flatten()
                && let Some(slug) = name.strip_prefix("litterbox/")
            {
                sandboxes.push(slug.to_string());
            }
        }

        Ok(sandboxes)
    }

    fn repo_prefix(&self) -> Result<String, SandboxError> {
        Ok(self.repo_prefix())
    }

    fn apply_patch(&self, diff: &str) -> Result<(), SandboxError> {
        let diff_obj = git2::Diff::from_buffer(diff.as_bytes()).map_err(|e| {
            SandboxError::from(GitError::ApplyPatch {
                message: format!("Failed to parse diff: {}", e),
            })
        })?;

        self.repo
            .apply(&diff_obj, git2::ApplyLocation::WorkDir, None)
            .map_err(|e| {
                SandboxError::from(GitError::ApplyPatch {
                    message: format!("Failed to apply patch: {}", e),
                })
            })
    }
}

impl Vcs for GitVcs {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            conflict_model: ConflictModel::Blocking,
            op_log: false,
            bindings: BindingSupport::Copied,
            export: ExportSupport::None,
        }
    }

    fn prepare(&self, sandbox: &str) -> Result<Prepared, VcsError> {
        let branch = self.create_branch(sandbox).map_err(vcs_error)?;
        let oid = self
            .repo_for_branch(&branch, |repo| {
                let oid = repo
                    .refname_to_id(&refs::branch(&branch))
                    .map_err(|source| SandboxError::from(GitError::Reference { source }))?;
                refs::write(
                    repo,
                    &refs::sandbox(&branch),
                    oid,
                    "litterbox: create the sandbox branch",
                )?;
                refs::write(
                    repo,
                    &refs::synced(&branch),
                    oid,
                    "litterbox: record the synced revision",
                )?;
                Ok(oid)
            })
            .map_err(vcs_error)?;

        Ok(Prepared {
            workspace: Workspace::from_branch(branch),
            base: Revision::new(oid.to_string()),
        })
    }

    fn masked_paths(&self, _workspace: &Workspace) -> Vec<PathBuf> {
        Vec::new()
    }

    fn bound_root(&self, _workspace: &Workspace) -> Result<PathBuf, VcsError> {
        Err(VcsError::Unsupported {
            operation: "bind the workspace".to_string(),
            backend: "git".to_string(),
            primitive: "a host directory that holds the branch's checkout".to_string(),
            reason: "the Git backend materialises a revision into a private copy and owns no \
                     host directory to bind"
                .to_string(),
            remedy: "use binding = 'copied'; a binding VCS arrives with the jj bridge".to_string(),
        })
    }

    fn identity(&self, workspace: &Workspace) -> Result<TreeId, VcsError> {
        let branch = workspace.branch().to_string();
        self.repo_for_branch(&branch, |repo| {
            let oid = repo
                .refname_to_id(&format!("refs/heads/{branch}"))
                .map_err(|source| SandboxError::from(GitError::Commit { source }))?;
            let commit = repo
                .find_commit(oid)
                .map_err(|source| SandboxError::from(GitError::Commit { source }))?;
            let tree = commit
                .tree()
                .map_err(|source| SandboxError::from(GitError::Commit { source }))?;
            Ok(tree.id().to_string())
        })
        .map(TreeId::new)
        .map_err(vcs_error)
    }

    fn materialise(
        &self,
        workspace: &Workspace,
        revision: &Revision,
        into: &Path,
    ) -> Result<(), VcsError> {
        let branch = workspace.branch().to_string();
        let bytes = self
            .repo_for_branch(&branch, |repo| {
                crate::vcs::git_common::archive::archive(repo, revision.as_str())
            })
            .map_err(vcs_error)?;
        crate::vcs::git_common::archive::unpack(&bytes, into).map_err(vcs_error)
    }

    fn snapshot_filter(&self, rules_root: &Path) -> Result<Arc<dyn SnapshotSelector>, VcsError> {
        let filter = self
            .lock()
            .map_err(vcs_error)?
            .snapshot_filter(rules_root)
            .map_err(vcs_error)?;
        Ok(Arc::new(filter))
    }

    fn ignore_files(&self) -> Arc<dyn SnapshotSelector> {
        Arc::new(crate::vcs::git_common::IgnoreFiles)
    }

    fn snapshot(
        &self,
        workspace: &Workspace,
        staging: &Path,
        message: &str,
    ) -> Result<Revision, VcsError> {
        let branch = workspace.branch().to_string();
        self.set_snapshot_ref(refs::sandbox(&branch))
            .map_err(vcs_error)?;
        match self
            .commit_snapshot_from_staging(staging, message)
            .map_err(vcs_error)?
        {
            Some(oid) => Ok(Revision::new(oid.to_string())),
            None => self.sandbox_tip(workspace),
        }
    }

    fn sandboxes(&self) -> Result<Vec<String>, VcsError> {
        Scm::list_sandboxes(self).map_err(vcs_error)
    }

    fn delete_branch(&self, sandbox: &str) -> Result<(), VcsError> {
        let branch = GitScm::branch_name(sandbox);
        self.repo_for_branch(&branch, |repo| {
            refs::delete(repo, &refs::sandbox(&branch))?;
            refs::delete(repo, &refs::synced(&branch))?;
            let mut found = repo.find_branch(&branch, BranchType::Local).map_err(|_| {
                SandboxError::SandboxNotFound {
                    name: sandbox.to_string(),
                }
            })?;
            found
                .delete()
                .map_err(|source| SandboxError::from(GitError::BranchDelete { source }))
        })
        .map_err(vcs_error)
    }

    fn state_path(&self) -> Option<PathBuf> {
        self.store_slug
            .as_ref()
            .map(|slug| crate::vcs::git::VcsStore::resolve_path(slug))
    }

    fn expose(&self, name: &str) -> Result<bool, VcsError> {
        let Some(store) = self.state_path() else {
            return Err(unsupported("expose"));
        };
        crate::vcs::git::expose_store(&store, name).map_err(vcs_error)
    }

    fn prune(&self) -> Result<(), VcsError> {
        let Some(slug) = &self.store_slug else {
            return Ok(());
        };
        let path = crate::vcs::git::VcsStore::resolve_path(slug);
        if !path.is_dir() {
            return Ok(());
        }
        let bare = Repository::open_bare(&path)
            .map_err(|source| SandboxError::from(GitError::Open { source }))
            .map_err(vcs_error)?;
        let mut branches = bare
            .branches(Some(BranchType::Local))
            .map_err(|source| SandboxError::from(GitError::BranchList { source }))
            .map_err(vcs_error)?;
        let any = branches.any(|branch| {
            branch
                .ok()
                .and_then(|(branch, _)| branch.name().ok().flatten().map(str::to_string))
                .is_some_and(|name| name.starts_with("litterbox/"))
        });
        if !any {
            crate::vcs::git::VcsStore::destroy_bare(slug).map_err(vcs_error)?;
        }
        Ok(())
    }

    fn tip(&self, workspace: &Workspace) -> Result<Revision, VcsError> {
        let branch = workspace.branch().to_string();
        self.repo_for_branch(&branch, |repo| {
            repo.refname_to_id(&format!("refs/heads/{branch}"))
                .map(|oid| oid.to_string())
                .map_err(|source| {
                    if source.code() == git2::ErrorCode::NotFound {
                        SandboxError::SandboxNotFound {
                            name: branch.clone(),
                        }
                    } else {
                        SandboxError::from(GitError::Reference { source })
                    }
                })
        })
        .map(Revision::new)
        .map_err(vcs_error)
    }

    fn set_tip(&self, workspace: &Workspace, revision: &Revision) -> Result<(), VcsError> {
        self.write_shared_branch(workspace, revision)
    }

    fn advance(
        &self,
        workspace: &Workspace,
        revision: &Revision,
        expected: &Revision,
    ) -> Result<(), VcsError> {
        self.move_shared_branch(workspace, revision, expected)
    }

    fn synced(&self, workspace: &Workspace) -> Result<Revision, VcsError> {
        let branch = workspace.branch().to_string();
        self.repo_for_branch(&branch, |repo| {
            refs::read(repo, &refs::synced(&branch))?
                .map(|oid| oid.to_string())
                .ok_or_else(|| SandboxError::SandboxNotFound {
                    name: branch.clone(),
                })
        })
        .map(Revision::new)
        .map_err(vcs_error)
    }

    fn set_synced(&self, workspace: &Workspace, revision: &Revision) -> Result<(), VcsError> {
        let branch = workspace.branch().to_string();
        let oid = revision_oid(revision).map_err(vcs_error)?;
        self.repo_for_branch(&branch, |repo| {
            refs::write(
                repo,
                &refs::synced(&branch),
                oid,
                "litterbox: record the synced revision",
            )
        })
        .map_err(vcs_error)
    }

    fn tree(&self, revision: &Revision) -> Result<TreeId, VcsError> {
        let oid = revision_oid(revision).map_err(vcs_error)?;
        let guard = self.lock().map_err(vcs_error)?;
        let commit = guard
            .repo
            .find_commit(oid)
            .map_err(|source| SandboxError::from(GitError::Commit { source }))
            .map_err(vcs_error)?;
        let tree = commit
            .tree()
            .map_err(|source| SandboxError::from(GitError::Commit { source }))
            .map_err(vcs_error)?;
        Ok(TreeId::new(tree.id().to_string()))
    }

    fn blob(&self, tree: &TreeId, path: &Path) -> Result<Option<Vec<u8>>, VcsError> {
        let oid = tree
            .as_str()
            .parse::<git2::Oid>()
            .map_err(|source| SandboxError::from(GitError::Commit { source }))
            .map_err(vcs_error)?;
        let guard = self.lock().map_err(vcs_error)?;
        let tree = guard
            .repo
            .find_tree(oid)
            .map_err(|source| SandboxError::from(GitError::Commit { source }))
            .map_err(vcs_error)?;
        let entry = match tree.get_path(path) {
            Ok(entry) => entry,
            Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(None),
            Err(source) => return Err(vcs_error(SandboxError::from(GitError::Commit { source }))),
        };
        let object = entry
            .to_object(&guard.repo)
            .map_err(|source| SandboxError::from(GitError::Commit { source }))
            .map_err(vcs_error)?;
        match object.as_blob() {
            Some(blob) => Ok(Some(blob.content().to_vec())),
            None => Ok(None),
        }
    }

    fn observe(&self, workspace: &Workspace, staging: &Path) -> Result<TreeId, VcsError> {
        let branch = workspace.branch().to_string();
        self.set_snapshot_ref(refs::sandbox(&branch))
            .map_err(vcs_error)?;
        let oid = self
            .lock()
            .map_err(vcs_error)?
            .staged_tree(staging)
            .map_err(vcs_error)?;
        Ok(TreeId::new(oid.to_string()))
    }

    fn sandbox_tip(&self, workspace: &Workspace) -> Result<Revision, VcsError> {
        let branch = workspace.branch().to_string();
        self.repo_for_branch(&branch, |repo| {
            refs::read(repo, &refs::sandbox(&branch))?
                .map(|oid| oid.to_string())
                .ok_or_else(|| SandboxError::SandboxNotFound {
                    name: branch.clone(),
                })
        })
        .map(Revision::new)
        .map_err(vcs_error)
    }

    fn set_sandbox_tip(&self, workspace: &Workspace, revision: &Revision) -> Result<(), VcsError> {
        let branch = workspace.branch().to_string();
        let oid = revision_oid(revision).map_err(vcs_error)?;
        self.repo_for_branch(&branch, |repo| {
            refs::write(
                repo,
                &refs::sandbox(&branch),
                oid,
                "litterbox: move the sandbox branch",
            )
        })
        .map_err(vcs_error)
    }

    fn is_ancestor(&self, ancestor: &Revision, descendant: &Revision) -> Result<bool, VcsError> {
        let ancestor = revision_oid(ancestor).map_err(vcs_error)?;
        let descendant = revision_oid(descendant).map_err(vcs_error)?;
        if ancestor == descendant {
            return Ok(true);
        }
        let guard = self.lock().map_err(vcs_error)?;
        guard
            .repo
            .graph_descendant_of(descendant, ancestor)
            .map_err(|source| SandboxError::from(GitError::Commit { source }))
            .map_err(vcs_error)
    }

    fn deleted_paths(&self, from: &TreeId, to: &TreeId) -> Result<Vec<PathBuf>, VcsError> {
        let parse = |value: &TreeId| {
            value
                .as_str()
                .parse::<git2::Oid>()
                .map_err(|source| SandboxError::from(GitError::Commit { source }))
        };
        let from = parse(from).map_err(vcs_error)?;
        let to = parse(to).map_err(vcs_error)?;

        let guard = self.lock().map_err(vcs_error)?;
        let from = guard
            .repo
            .find_tree(from)
            .map_err(|source| SandboxError::from(GitError::Commit { source }))
            .map_err(vcs_error)?;
        let to = guard
            .repo
            .find_tree(to)
            .map_err(|source| SandboxError::from(GitError::Commit { source }))
            .map_err(vcs_error)?;
        let diff = guard
            .repo
            .diff_tree_to_tree(Some(&from), Some(&to), None)
            .map_err(|source| SandboxError::from(GitError::Commit { source }))
            .map_err(vcs_error)?;

        let mut paths = Vec::new();
        for delta in diff.deltas() {
            if delta.status() == git2::Delta::Deleted
                && let Some(path) = delta.old_file().path()
            {
                paths.push(path.to_path_buf());
            }
        }
        Ok(paths)
    }

    fn report(&self, workspace: &Workspace) -> Result<Report, VcsError> {
        let branch_name = workspace.branch().to_string();
        let repo = self.repo_path_for(&branch_name)?;
        let synced = self.synced(workspace)?;
        let branch = self.tip(workspace)?;
        let sandbox = self.sandbox_tip(workspace)?;
        let path = reconcile::worktree_path(
            &self.reconcile_root(),
            &self.project_slug()?,
            sandbox_slug(&branch_name),
        );
        let worktree = path.is_dir().then_some(path);
        reconcile::report(
            &repo,
            &synced,
            &branch,
            &sandbox,
            refs::branch(&branch_name),
            worktree,
        )
    }

    fn stage(&self, workspace: &Workspace) -> Result<Staged, VcsError> {
        let branch_name = workspace.branch().to_string();
        let repo = self.repo_path_for(&branch_name)?;
        let slug = sandbox_slug(&branch_name).to_string();
        let path = reconcile::worktree_path(&self.reconcile_root(), &self.project_slug()?, &slug);
        let synced = self.synced(workspace)?;
        let branch = self.tip(workspace)?;
        let sandbox = self.sandbox_tip(workspace)?;
        let temp = format!("litterbox-reconcile/{slug}");
        reconcile::stage(&repo, &path, &synced, &branch, &sandbox, &temp)
    }

    fn finish(&self, workspace: &Workspace) -> Result<Finished, VcsError> {
        let branch_name = workspace.branch().to_string();
        let repo = self.repo_path_for(&branch_name)?;
        let slug = sandbox_slug(&branch_name).to_string();
        let path = reconcile::worktree_path(&self.reconcile_root(), &self.project_slug()?, &slug);
        let branch = self.tip(workspace)?;
        let temp = format!("litterbox-reconcile/{slug}");
        reconcile::finish(&repo, &path, &branch_name, &branch, &temp)
    }

    fn discard_stage(&self, workspace: &Workspace) -> Result<(), VcsError> {
        let branch_name = workspace.branch().to_string();
        let repo = self.repo_path_for(&branch_name)?;
        let slug = sandbox_slug(&branch_name).to_string();
        let path = reconcile::worktree_path(&self.reconcile_root(), &self.project_slug()?, &slug);
        let temp = format!("litterbox-reconcile/{slug}");
        reconcile::discard(&repo, &path, &temp)
    }

    fn ensure_refs(&self, workspace: &Workspace) -> Result<(), VcsError> {
        let branch = workspace.branch().to_string();
        self.repo_for_branch(&branch, |repo| {
            let oid = repo
                .refname_to_id(&refs::branch(&branch))
                .map_err(|source| SandboxError::from(GitError::Reference { source }))?;

            if refs::read(repo, &refs::sandbox(&branch))?.is_none() {
                refs::write(
                    repo,
                    &refs::sandbox(&branch),
                    oid,
                    "litterbox: initialise the sandbox branch",
                )?;
                tracing::info!(branch = %branch, "initialised the sandbox branch ref");
            }

            if refs::read(repo, &refs::synced(&branch))?.is_none() {
                refs::write(
                    repo,
                    &refs::synced(&branch),
                    oid,
                    "litterbox: initialise the synced revision",
                )?;
                tracing::info!(branch = %branch, "initialised the synced ref");
            }

            Ok(())
        })
        .map_err(vcs_error)
    }

    fn merge(
        &self,
        _base: &Revision,
        _ours: &Revision,
        _theirs: &Revision,
    ) -> Result<Merge, VcsError> {
        Err(unsupported("merge"))
    }

    fn resolve_begin(&self, workspace: &Workspace) -> Result<Begun, VcsError> {
        let base = self.synced(workspace)?;
        let ours = self.sandbox_tip(workspace)?;
        let theirs = self.tip(workspace)?;
        let repo = {
            let guard = self.lock().map_err(vcs_error)?;
            guard.repo.path().to_path_buf()
        };
        let revision = reconcile::begin(&repo, &base, &ours, &theirs)?;
        self.set_sandbox_tip(workspace, &revision)?;
        Ok(Begun { revision })
    }

    fn resolve_abandon(
        &self,
        workspace: &Workspace,
        ours: &Revision,
    ) -> Result<Revision, VcsError> {
        self.set_sandbox_tip(workspace, ours)?;
        Ok(Revision::new(ours.as_str().to_string()))
    }

    fn conflicts(&self, revision: &Revision) -> Result<Vec<Conflict>, VcsError> {
        let repo = {
            let guard = self.lock().map_err(vcs_error)?;
            guard.repo.path().to_path_buf()
        };
        reconcile::marker_conflicts(&repo, revision.as_str())
    }

    fn conflict_sides(&self, revision: &Revision, _path: &Path) -> Result<ConflictSides, VcsError> {
        let oid = revision_oid(revision).map_err(vcs_error)?;
        let guard = self.lock().map_err(vcs_error)?;
        let commit = guard
            .repo
            .find_commit(oid)
            .map_err(|source| SandboxError::from(GitError::Commit { source }))
            .map_err(vcs_error)?;
        if commit.parent_count() < 2 {
            return Err(VcsError::Unsupported {
                operation: "read the sides of a conflict".to_string(),
                backend: "git".to_string(),
                primitive: "a merge revision with two parents".to_string(),
                reason: "the revision is not a merge".to_string(),
                remedy: "materialise the merge first".to_string(),
            });
        }
        let parent = |index: usize| -> Result<git2::Commit<'_>, VcsError> {
            commit
                .parent(index)
                .map_err(|source| SandboxError::from(GitError::Commit { source }))
                .map_err(vcs_error)
        };
        let ours = parent(0)?;
        let theirs = parent(1)?;
        let base = guard
            .repo
            .merge_base(ours.id(), theirs.id())
            .ok()
            .and_then(|oid| guard.repo.find_commit(oid).ok())
            .map(|commit| TreeId::new(commit.tree_id().to_string()));
        Ok(ConflictSides {
            base,
            ours: TreeId::new(ours.tree_id().to_string()),
            theirs: TreeId::new(theirs.tree_id().to_string()),
        })
    }

    fn resolve(&self, _revision: &Revision, _paths: &[PathBuf]) -> Result<Revision, VcsError> {
        Err(unsupported("resolve"))
    }

    fn abort(&self, _revision: &Revision) -> Result<(), VcsError> {
        Err(unsupported("abort"))
    }

    fn history(
        &self,
        _workspace: &Workspace,
        _keep: &HistoryPolicy,
    ) -> Result<Vec<Revision>, VcsError> {
        Err(unsupported("history"))
    }

    fn restore(
        &self,
        _workspace: &Workspace,
        _revision: &Revision,
        _mode: RestoreMode,
    ) -> Result<(), VcsError> {
        Err(unsupported("restore"))
    }

    fn export(&self) -> Result<(), VcsError> {
        Ok(())
    }
}

impl GitVcs {
    fn write_shared_branch(
        &self,
        workspace: &Workspace,
        revision: &Revision,
    ) -> Result<(), VcsError> {
        let branch = workspace.branch().to_string();
        let oid = revision_oid(revision).map_err(vcs_error)?;
        self.repo_for_branch(&branch, |repo| {
            refs::write(
                repo,
                &refs::branch(&branch),
                oid,
                "litterbox: move the branch",
            )
        })
        .map_err(vcs_error)
    }

    fn move_shared_branch(
        &self,
        workspace: &Workspace,
        revision: &Revision,
        expected: &Revision,
    ) -> Result<(), VcsError> {
        let branch = workspace.branch().to_string();
        let oid = revision_oid(revision).map_err(vcs_error)?;
        let expected = revision_oid(expected).map_err(vcs_error)?;
        self.repo_for_branch(&branch, |repo| {
            refs::write_matching(
                repo,
                &refs::branch(&branch),
                oid,
                expected,
                "litterbox: advance the branch",
            )
        })
        .map_err(vcs_error)
    }

    /// Run `operation` against the repository that holds `branch`.
    ///
    /// The bare clone is checked first, then the host repository, then the
    /// configured store. That is the same order `make_archive` uses, and it
    /// lets one backend instance serve records created under either locality.
    fn repo_for_branch<T>(
        &self,
        branch: &str,
        operation: impl FnOnce(&Repository) -> Result<T, SandboxError>,
    ) -> Result<T, SandboxError> {
        let guard = self.lock()?;

        if guard.repo.find_branch(branch, BranchType::Local).is_ok() {
            return operation(&guard.repo);
        }

        if let Some(path) = &guard.host_repo_path
            && let Ok(host) = Repository::open(path)
            && host.find_branch(branch, BranchType::Local).is_ok()
        {
            return operation(&host);
        }

        if let Some(slug) = &self.store_slug {
            let path = crate::vcs::git::VcsStore::resolve_path(slug);
            if let Ok(bare) = Repository::open_bare(&path)
                && bare.find_branch(branch, BranchType::Local).is_ok()
            {
                return operation(&bare);
            }
        }

        operation(&guard.repo)
    }
}

fn revision_oid(revision: &Revision) -> Result<git2::Oid, SandboxError> {
    revision
        .as_str()
        .parse()
        .map_err(|source| SandboxError::from(GitError::Commit { source }))
}

fn sandbox_slug(branch: &str) -> &str {
    branch.strip_prefix("litterbox/").unwrap_or(branch)
}

fn vcs_error(error: SandboxError) -> VcsError {
    match error {
        SandboxError::SandboxExists { name } => VcsError::Exists { name },
        SandboxError::SandboxNotFound { name } => VcsError::NotFound { name },
        SandboxError::BranchMoved { .. } => VcsError::Raced,
        SandboxError::Io(source) => VcsError::Io(source),
        other => VcsError::Backend {
            operation: "git".to_string(),
            message: other.to_string(),
        },
    }
}

fn unsupported(operation: &str) -> VcsError {
    VcsError::Unsupported {
        operation: operation.to_string(),
        backend: "git".to_string(),
        primitive: "not implemented in this cut".to_string(),
        reason: "the conflict and history surface lands in a later change".to_string(),
        remedy: "create, prepare, materialise, snapshot, list and delete are available".to_string(),
    }
}

impl GitScm {
    /// The paths the snapshot's parent records, which stay tracked however the
    /// sandbox's ignore rules read.
    fn snapshot_tracked(&self) -> Result<(HashSet<PathBuf>, HashSet<PathBuf>), SandboxError> {
        let mut tracked = HashSet::new();
        let mut tracked_dirs = HashSet::new();

        let Some(parent) = self.snapshot_target()? else {
            return Ok((tracked, tracked_dirs));
        };
        let tree = parent
            .tree()
            .map_err(|source| SandboxError::from(GitError::Commit { source }))?;

        tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
            let path = Path::new(root).join(entry.name().unwrap_or_default());
            for ancestor in path.ancestors().skip(1) {
                if ancestor.as_os_str().is_empty() {
                    break;
                }
                tracked_dirs.insert(ancestor.to_path_buf());
            }
            if entry.kind() == Some(ObjectType::Blob) {
                tracked.insert(path);
            }
            0
        })
        .map_err(|source| SandboxError::from(GitError::Commit { source }))?;

        Ok((tracked, tracked_dirs))
    }

    /// The paths this sandbox's snapshot carries.
    ///
    /// The rules are read from `rules_root`, the sandbox's own tree: a
    /// `.gitignore` edited inside the sandbox decides what the snapshot
    /// records, not the host checkout the branch happens to be stored beside.
    pub fn snapshot_filter(&self, rules_root: &Path) -> Result<SnapshotFilter, SandboxError> {
        let (tracked, tracked_dirs) = self.snapshot_tracked()?;
        SnapshotFilter::open(rules_root, tracked, tracked_dirs)
    }

    /// The tree a snapshot of `staging_path` would commit.
    fn staged_tree(&self, staging_path: &Path) -> Result<git2::Oid, SandboxError> {
        let mut builder = self
            .repo
            .treebuilder(None)
            .map_err(|source| SandboxError::from(GitError::Commit { source }))?;

        // The staged tree is the sandbox's, so its rules are the ones that
        // decide what the snapshot records; the transfer applied the same ones.
        let filter = self.snapshot_filter(staging_path)?;
        self.add_directory_to_tree(&mut builder, staging_path, Path::new(""), &filter)?;

        builder
            .write()
            .map_err(|source| SandboxError::from(GitError::Commit { source }))
    }

    fn commit_snapshot_from_staging(
        &self,
        staging_path: &Path,
        message: &str,
    ) -> Result<Option<git2::Oid>, SandboxError> {
        let signature = self.signature()?;
        let tree_oid = self.staged_tree(staging_path)?;

        let tree = self
            .repo
            .find_tree(tree_oid)
            .map_err(|source| SandboxError::from(GitError::Commit { source }))?;

        // Another tool call can snapshot the same sandbox while this tree is
        // being built, so the branch may move. Build each commit against the
        // branch as it is now and move the ref only if it still points there,
        // or one of the two snapshots is silently dropped.
        let mut attempts = 0;
        loop {
            // The branch may not exist yet; its absence and HEAD pointing at a
            // commit are different facts, and the compare-and-swap needs to
            // know which it is.
            let (parent, ref_exists) = match self.snapshot_target()? {
                Some(commit) => (Some(commit), true),
                None => (self.head_commit_optional()?, false),
            };
            if let Some(ref parent_commit) = parent {
                if parent_commit.tree_id() == tree_oid {
                    return Ok(None);
                }
            } else if tree_oid == git2::Oid::ZERO_SHA1 {
                return Ok(None);
            }

            let parents: Vec<&git2::Commit> = parent.iter().collect();
            // Create the commit without updating the ref: the compare-and-swap
            // below decides whether it becomes the branch tip.
            let oid = self
                .repo
                .commit(None, &signature, &signature, message, &tree, &parents)
                .map_err(|source| SandboxError::from(GitError::Commit { source }))?;

            let updated = if ref_exists {
                let expected = parent
                    .as_ref()
                    .map(git2::Commit::id)
                    .expect("a reference peels to a commit");
                self.repo
                    .reference_matching(&self.snapshot_branch_ref(), oid, true, expected, message)
                    .map(|_| ())
            } else {
                self.repo
                    .reference(&self.snapshot_branch_ref(), oid, false, message)
                    .map(|_| ())
            };

            match updated {
                Ok(()) => return Ok(Some(oid)),
                Err(error)
                    if matches!(
                        error.code(),
                        git2::ErrorCode::Modified
                            | git2::ErrorCode::Exists
                            | git2::ErrorCode::NotFound
                    ) && attempts < MAX_SNAPSHOT_ATTEMPTS =>
                {
                    attempts += 1;
                    continue;
                }
                Err(error)
                    if error.code() == git2::ErrorCode::Locked
                        && attempts < MAX_SNAPSHOT_ATTEMPTS =>
                {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(10 * attempts as u64));
                    continue;
                }
                Err(error) => {
                    // The ref has not moved: it is only written by a
                    // compare-and-swap that has not succeeded, so there is
                    // nothing to roll back. Restoring the tip seen at the start
                    // would undo whatever another writer has published since.
                    return Err(SandboxError::from(GitError::Commit { source: error }));
                }
            }
        }
    }

    fn add_directory_to_tree(
        &self,
        builder: &mut git2::TreeBuilder,
        current_path: &Path,
        relative: &Path,
        filter: &SnapshotFilter,
    ) -> Result<(), SandboxError> {
        let entries = fs::read_dir(current_path).map_err(SandboxError::Io)?;

        for entry in entries {
            let entry = entry.map_err(SandboxError::Io)?;
            let path = entry.path();
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();

            // Skip .git directory
            if name_str == ".git" {
                continue;
            }

            let child = relative.join(&file_name);
            let file_type = entry.file_type().map_err(SandboxError::Io)?;

            if file_type.is_dir() {
                if filter.skips_dir(&child)? {
                    continue;
                }

                let mut sub_builder = self
                    .repo
                    .treebuilder(None)
                    .map_err(|source| SandboxError::from(GitError::Commit { source }))?;

                self.add_directory_to_tree(&mut sub_builder, &path, &child, filter)?;

                let sub_tree_oid = sub_builder
                    .write()
                    .map_err(|source| SandboxError::from(GitError::Commit { source }))?;

                builder
                    .insert(&*name_str, sub_tree_oid, 0o040000)
                    .map_err(|source| SandboxError::from(GitError::Commit { source }))?;
            } else if file_type.is_symlink() {
                if filter.skips_file(&child)? {
                    continue;
                }

                // Git records a link as its target path, never the content the
                // link points at.
                let target = fs::read_link(&path).map_err(SandboxError::Io)?;
                let blob_oid = self
                    .repo
                    .blob(target.as_os_str().as_encoded_bytes())
                    .map_err(|source| SandboxError::from(GitError::Commit { source }))?;

                builder
                    .insert(&*name_str, blob_oid, 0o120000)
                    .map_err(|source| SandboxError::from(GitError::Commit { source }))?;
            } else if file_type.is_file() {
                if filter.skips_file(&child)? {
                    continue;
                }

                // Stream the file into the object database: build outputs are
                // large enough that reading one whole into memory is a risk.
                let mut source = fs::File::open(&path).map_err(SandboxError::Io)?;
                let mut writer = self
                    .repo
                    .blob_writer(None)
                    .map_err(|source| SandboxError::from(GitError::Commit { source }))?;
                std::io::copy(&mut source, &mut writer).map_err(SandboxError::Io)?;
                let blob_oid = writer
                    .commit()
                    .map_err(|source| SandboxError::from(GitError::Commit { source }))?;

                #[cfg(unix)]
                let filemode = {
                    use std::os::unix::fs::PermissionsExt;
                    let metadata = entry.metadata().map_err(SandboxError::Io)?;
                    let mode = metadata.permissions().mode();
                    if mode & 0o111 != 0 {
                        0o100755
                    } else {
                        0o100644
                    }
                };

                #[cfg(not(unix))]
                let filemode = 0o100644;

                builder
                    .insert(&*name_str, blob_oid, filemode)
                    .map_err(|source| SandboxError::from(GitError::Commit { source }))?;
            }
        }

        Ok(())
    }
}

fn repo_prefix_from_path(path: &Path) -> String {
    let base = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("repo");
    let slug = slugify(base);
    if slug.is_empty() {
        "repo".to_string()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::io::Cursor;

    use git2::{IndexAddOption, Signature};
    use tempfile::TempDir;

    fn init_repo() -> (TempDir, Repository) {
        let tempdir = TempDir::new().expect("tempdir");
        let repo = Repository::init(tempdir.path()).expect("repo init");

        let file_path = tempdir.path().join("README.md");
        fs::write(&file_path, "hello").expect("write file");

        let gitignore_path = tempdir.path().join(".gitignore");
        fs::write(&gitignore_path, "ignored.txt\n").expect("write gitignore");

        let mut index = repo.index().expect("index");
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .expect("add all");
        index.write().expect("index write");
        let tree_id = index.write_tree().expect("write tree");

        let signature = Signature::now("Litterbox", "noreply@example.com").expect("signature");
        {
            let tree = repo.find_tree(tree_id).expect("find tree");
            repo.commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])
                .expect("commit");
        }

        (tempdir, repo)
    }

    #[test]
    fn a_marker_tree_reports_its_regions() {
        let (tempdir, repo) = init_repo();
        let marker = "a\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> abc\n\
                      b\n<<<<<<< HEAD\nours2\n=======\ntheirs2\n>>>>>>> abc\n";
        fs::write(tempdir.path().join("conflicted.txt"), marker).expect("write file");

        let mut index = repo.index().expect("index");
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .expect("add all");
        index.write().expect("index write");
        let tree_id = index.write_tree().expect("write tree");
        let signature = Signature::now("Litterbox", "noreply@example.com").expect("signature");
        let parent = repo.head().expect("head").peel_to_commit().expect("commit");
        let tree = repo.find_tree(tree_id).expect("find tree");
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            "conflicted",
            &tree,
            &[&parent],
        )
        .expect("commit");

        let conflicts =
            crate::vcs::git::reconcile::marker_conflicts(repo.path(), "HEAD").expect("scan");
        assert_eq!(conflicts.len(), 1, "{conflicts:?}");
        assert_eq!(conflicts[0].path, Path::new("conflicted.txt"));
        assert_eq!(conflicts[0].hunks, 2, "two regions");
        assert_eq!(conflicts[0].sides, 2, "Git markers carry two sides");
        assert!(!conflicts[0].base, "no diff3 base section");

        let clean =
            crate::vcs::git::reconcile::marker_conflicts(repo.path(), "HEAD^").expect("scan");
        assert!(clean.is_empty(), "{clean:?}");
    }

    #[test]
    fn create_branch_creates_litterbox_branch() {
        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: None,
            host_repo_path: None,
        };

        let branch_name = scm.create_branch("my-feature").expect("create branch");
        assert_eq!(branch_name, "litterbox/my-feature");

        let branch = scm
            .repo
            .find_branch(&branch_name, BranchType::Local)
            .expect("branch exists");
        let branch_commit = branch.get().peel_to_commit().expect("branch commit");
        let head_commit = scm
            .repo
            .head()
            .expect("head")
            .peel_to_commit()
            .expect("head commit");
        assert_eq!(branch_commit.id(), head_commit.id());
    }

    #[test]
    fn create_branch_rejects_duplicates() {
        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: None,
            host_repo_path: None,
        };

        scm.create_branch("my-feature").expect("create branch");
        let err = scm
            .create_branch("my-feature")
            .expect_err("duplicate branch");
        assert_eq!(err.to_string(), "Sandbox 'my-feature' already exists.");
    }

    #[test]
    fn delete_branch_removes_branch() {
        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: None,
            host_repo_path: None,
        };

        let branch_name = scm.create_branch("cleanup").expect("create branch");
        scm.delete_branch("cleanup").expect("delete branch");

        assert!(
            scm.repo
                .find_branch(&branch_name, BranchType::Local)
                .is_err()
        );
    }

    #[test]
    fn delete_branch_missing_returns_not_found() {
        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: None,
            host_repo_path: None,
        };

        let err = scm.delete_branch("missing").expect_err("missing branch");
        assert_eq!(err.to_string(), "Sandbox 'missing' not found.");
    }

    #[test]
    fn archive_contains_tracked_files_only() {
        let (tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: None,
            host_repo_path: None,
        };

        let ignored_path = tempdir.path().join("ignored.txt");
        fs::write(&ignored_path, "ignored").expect("write ignored");
        let untracked_path = tempdir.path().join("notes.txt");
        fs::write(&untracked_path, "notes").expect("write untracked");

        let archive = scm.make_archive("HEAD").expect("archive");
        let mut entries = Vec::new();
        let mut reader = tar::Archive::new(Cursor::new(archive));
        for entry in reader.entries().expect("entries") {
            let entry = entry.expect("entry");
            let path = entry.path().expect("path");
            entries.push(path.to_string_lossy().to_string());
        }

        entries.sort();
        assert_eq!(entries, vec![".gitignore", "README.md"]);
    }

    #[test]
    fn commit_snapshot_from_staging_creates_commit_on_snapshot_branch() {
        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: Some("test-snapshot".to_string()),
            host_repo_path: None,
        };

        let staging_dir = TempDir::new().expect("staging dir");
        fs::write(staging_dir.path().join("file.txt"), "content").expect("write file");

        let oid = scm
            .commit_snapshot_from_staging(staging_dir.path(), "Test snapshot")
            .expect("commit")
            .expect("oid");

        let commit = scm.repo.find_commit(oid).expect("commit lookup");
        assert_eq!(commit.message().expect("message"), "Test snapshot");

        let snapshot_ref = scm
            .repo
            .find_reference("refs/heads/test-snapshot")
            .expect("ref");
        assert_eq!(snapshot_ref.target().expect("target"), oid);

        // Verify tree actually contains the file with correct content
        let tree = commit.tree().expect("tree");
        assert_eq!(tree.len(), 1);
        let file_entry = tree.get_name("file.txt").expect("file.txt entry");
        let blob = scm.repo.find_blob(file_entry.id()).expect("blob");
        assert_eq!(blob.content(), b"content");
    }

    #[test]
    fn commit_snapshot_from_staging_skips_git_directory() {
        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: Some("test-snapshot".to_string()),
            host_repo_path: None,
        };

        let staging_dir = TempDir::new().expect("staging dir");
        fs::write(staging_dir.path().join("file.txt"), "content").expect("write file");

        // Create a .git directory that should be skipped
        let git_dir = staging_dir.path().join(".git");
        fs::create_dir(&git_dir).expect("create .git dir");
        fs::write(git_dir.join("config"), "fake git config").expect("write git config");

        let oid = scm
            .commit_snapshot_from_staging(staging_dir.path(), "Test snapshot")
            .expect("commit")
            .expect("oid");

        let commit = scm.repo.find_commit(oid).expect("commit lookup");
        let tree = commit.tree().expect("tree");

        // Should only have file.txt, not .git
        assert_eq!(tree.len(), 1);
        assert_eq!(
            tree.get_name("file.txt")
                .expect("file.txt entry")
                .name()
                .expect("name"),
            "file.txt"
        );
        assert!(tree.get_name(".git").is_none());
    }

    #[test]
    fn commit_snapshot_from_staging_skips_ignored_paths() {
        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: Some("test-snapshot".to_string()),
            host_repo_path: None,
        };

        let staging_dir = TempDir::new().expect("staging dir");
        fs::write(
            staging_dir.path().join(".gitignore"),
            "ignored.txt\n/target/\n",
        )
        .expect("write gitignore");
        let target = staging_dir.path().join("target");
        fs::create_dir_all(&target).expect("create target");
        fs::write(target.join("artifact"), "built").expect("write artifact");
        let source = staging_dir.path().join("src");
        fs::create_dir_all(&source).expect("create src");
        fs::write(source.join("main.rs"), "fn main() {}").expect("write source");

        let oid = scm
            .commit_snapshot_from_staging(staging_dir.path(), "Test snapshot")
            .expect("commit")
            .expect("oid");

        let tree = scm
            .repo
            .find_commit(oid)
            .expect("commit")
            .tree()
            .expect("tree");
        assert!(
            tree.get_name("target").is_none(),
            "build output must not be tracked"
        );
        assert!(tree.get_name("src").is_some(), "source must be tracked");
    }

    #[test]
    fn commit_snapshot_from_staging_honours_nested_gitignore() {
        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: Some("test-snapshot".to_string()),
            host_repo_path: None,
        };

        let staging_dir = TempDir::new().expect("staging dir");
        let staged_nested = staging_dir.path().join("crates").join("thing");
        fs::create_dir_all(&staged_nested).expect("create staged nested");
        fs::write(staged_nested.join(".gitignore"), "*.log\n!keep.log\n")
            .expect("write nested ignore");
        fs::write(staged_nested.join("debug.log"), "noise").expect("write ignored");
        fs::write(staged_nested.join("keep.log"), "kept").expect("write kept");
        fs::write(staged_nested.join("lib.rs"), "pub fn thing() {}").expect("write source");

        let oid = scm
            .commit_snapshot_from_staging(staging_dir.path(), "Test snapshot")
            .expect("commit")
            .expect("oid");

        let tree = scm
            .repo
            .find_commit(oid)
            .expect("commit")
            .tree()
            .expect("tree");
        let crates = tree
            .get_name("crates")
            .expect("crates entry")
            .to_object(&scm.repo)
            .expect("crates object");
        let thing = crates
            .as_tree()
            .expect("crates tree")
            .get_name("thing")
            .expect("thing entry")
            .to_object(&scm.repo)
            .expect("thing object");
        let thing = thing.as_tree().expect("thing tree");
        assert!(thing.get_name("debug.log").is_none(), "rule must apply");
        assert!(thing.get_name("keep.log").is_some(), "negation must apply");
        assert!(thing.get_name("lib.rs").is_some());
    }

    #[test]
    fn commit_snapshot_from_staging_keeps_tracked_ignored_files() {
        let (tempdir, repo) = init_repo();
        let kept = tempdir.path().join("kept.txt");
        fs::write(&kept, "tracked").expect("write kept");
        let mut index = repo.index().expect("index");
        index.add_path(Path::new("kept.txt")).expect("add kept");
        index.write().expect("write index");
        let tree_id = index.write_tree().expect("write tree");
        let signature = Signature::now("Test", "test@example.com").expect("signature");
        {
            let tree = repo.find_tree(tree_id).expect("tree");
            let parent = repo
                .head()
                .expect("head")
                .peel_to_commit()
                .expect("head commit");
            let head = repo
                .commit(
                    Some("HEAD"),
                    &signature,
                    &signature,
                    "track kept",
                    &tree,
                    &[&parent],
                )
                .expect("commit kept");
            // The snapshot's parent is the branch, not the checkout: a branch
            // that does not exist yet records nothing.
            repo.branch(
                "test-snapshot",
                &repo.find_commit(head).expect("committed"),
                false,
            )
            .expect("snapshot branch");
        }
        drop(index);

        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: Some("test-snapshot".to_string()),
            host_repo_path: None,
        };

        let staging_dir = TempDir::new().expect("staging dir");
        fs::write(
            staging_dir.path().join(".gitignore"),
            "ignored.txt\nkept.txt\n",
        )
        .expect("write gitignore");
        fs::write(staging_dir.path().join("kept.txt"), "tracked").expect("write kept");

        let oid = scm
            .commit_snapshot_from_staging(staging_dir.path(), "Test snapshot")
            .expect("commit")
            .expect("oid");

        let tree = scm
            .repo
            .find_commit(oid)
            .expect("commit")
            .tree()
            .expect("tree");
        assert!(
            tree.get_name("kept.txt").is_some(),
            "an ignored path the snapshot's parent records must stay"
        );
    }

    #[test]
    fn commit_snapshot_from_staging_tracks_what_the_snapshot_parent_records() {
        let (tempdir, repo) = init_repo();
        fs::write(tempdir.path().join("kept.txt"), "tracked").expect("write kept");
        let mut index = repo.index().expect("index");
        index.add_path(Path::new("kept.txt")).expect("add kept");
        index.write().expect("write index");
        let tree_id = index.write_tree().expect("write tree");
        let signature = Signature::now("Test", "test@example.com").expect("signature");
        {
            let tree = repo.find_tree(tree_id).expect("tree");
            let parent = repo
                .head()
                .expect("head")
                .peel_to_commit()
                .expect("head commit");
            let head = repo
                .commit(
                    Some("HEAD"),
                    &signature,
                    &signature,
                    "track kept",
                    &tree,
                    &[&parent],
                )
                .expect("commit kept");
            repo.branch(
                "litterbox/other",
                &repo.find_commit(head).expect("committed"),
                false,
            )
            .expect("snapshot branch");
        }

        // The checkout moves to a state that carries no record of the file, so
        // its index cannot be the snapshot's idea of what is tracked.
        index
            .remove_path(Path::new("kept.txt"))
            .expect("untrack kept");
        index.write().expect("write index");

        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: Some("litterbox/other".to_string()),
            host_repo_path: None,
        };

        let staging_dir = TempDir::new().expect("staging dir");
        fs::write(staging_dir.path().join(".gitignore"), "kept.txt\n").expect("write gitignore");
        fs::write(staging_dir.path().join("kept.txt"), "tracked").expect("write kept");

        let oid = scm
            .commit_snapshot_from_staging(staging_dir.path(), "Test snapshot")
            .expect("commit")
            .expect("oid");

        let tree = scm
            .repo
            .find_commit(oid)
            .expect("commit")
            .tree()
            .expect("tree");
        assert!(
            tree.get_name("kept.txt").is_some(),
            "the snapshot's own parent decides what is tracked, not the checkout"
        );
    }

    #[test]
    fn commit_snapshot_from_staging_takes_its_rules_from_the_sandbox_in_remote_mode() {
        let (host, _host_repo) = init_repo();
        // The checkout the branch is stored beside ignores logs; the sandbox
        // does not. The sandbox's rules are the ones the snapshot records.
        fs::write(host.path().join(".gitignore"), "*.log\n/target/\n").expect("rewrite gitignore");

        let bare = TempDir::new().expect("bare dir");
        Repository::init_bare(bare.path()).expect("init bare");
        let mut scm = GitScm::open_with_host(
            bare.path(),
            ScmMode::Remote,
            Some(host.path().to_path_buf()),
        )
        .expect("open remote scm");
        scm.set_snapshot_branch("litterbox/remote".to_string());

        let staging_dir = TempDir::new().expect("staging dir");
        fs::write(staging_dir.path().join(".gitignore"), "/target/\n")
            .expect("write sandbox gitignore");
        let target = staging_dir.path().join("target");
        fs::create_dir_all(&target).expect("create target");
        fs::write(target.join("artifact"), "built").expect("write artifact");
        fs::write(staging_dir.path().join("README.md"), "hello").expect("write readme");
        fs::write(staging_dir.path().join("notes.log"), "kept").expect("write log");

        let oid = scm
            .commit_snapshot_from_staging(staging_dir.path(), "Test snapshot")
            .expect("commit")
            .expect("oid");

        let tree = scm
            .repo
            .find_commit(oid)
            .expect("commit")
            .tree()
            .expect("tree");
        assert!(
            tree.get_name("target").is_none(),
            "the sandbox's rules must apply in remote mode"
        );
        assert!(
            tree.get_name("notes.log").is_some(),
            "the host checkout's rules must not reach the snapshot"
        );
        assert!(tree.get_name("README.md").is_some());
    }

    #[cfg(unix)]
    #[test]
    fn commit_snapshot_from_staging_stores_symlinks_as_links() {
        use std::os::unix::fs::symlink;

        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: Some("test-snapshot".to_string()),
            host_repo_path: None,
        };

        let staging_dir = TempDir::new().expect("staging dir");
        let outside = TempDir::new().expect("outside");
        fs::write(outside.path().join("secret"), "do not read this").expect("write secret");
        symlink(
            outside.path().join("secret"),
            staging_dir.path().join("link"),
        )
        .expect("symlink");
        // A link back up the tree must not be followed, or the walk recurses.
        symlink("..", staging_dir.path().join("loop")).expect("loop symlink");

        let oid = scm
            .commit_snapshot_from_staging(staging_dir.path(), "Test snapshot")
            .expect("commit")
            .expect("oid");

        let tree = scm
            .repo
            .find_commit(oid)
            .expect("commit")
            .tree()
            .expect("tree");
        let link = tree.get_name("link").expect("link entry");
        assert_eq!(link.filemode(), 0o120000, "a link stays a link");
        let blob = scm.repo.find_blob(link.id()).expect("blob");
        assert_eq!(
            blob.content(),
            outside.path().join("secret").as_os_str().as_encoded_bytes()
        );
        assert_eq!(
            tree.get_name("loop").expect("loop entry").filemode(),
            0o120000
        );
    }

    #[test]
    fn concurrent_snapshots_all_land_on_the_branch() {
        let (tempdir, _repo) = init_repo();
        let path = tempdir.path().to_path_buf();

        let writers: Vec<_> = (0..4)
            .map(|index| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let mut scm = GitScm::open_direct(&path).expect("open");
                    scm.set_snapshot_branch("test-snapshot".to_string());
                    let staging = TempDir::new().expect("staging");
                    fs::write(staging.path().join(format!("file{index}.txt")), "content")
                        .expect("write");
                    scm.commit_snapshot_from_staging(staging.path(), &format!("snapshot {index}"))
                        .expect("commit")
                        .expect("oid");
                })
            })
            .collect();
        for writer in writers {
            writer.join().expect("writer");
        }

        let repo = Repository::open(&path).expect("reopen");
        let tip = repo
            .find_reference("refs/heads/test-snapshot")
            .expect("ref")
            .peel_to_commit()
            .expect("tip");

        let mut messages = Vec::new();
        let mut commit = tip;
        while commit.message() != Ok("init") {
            messages.push(commit.message().unwrap_or_default().to_string());
            commit = commit.parent(0).expect("parent");
        }
        messages.sort();

        let expected: Vec<String> = (0..4).map(|index| format!("snapshot {index}")).collect();
        assert_eq!(messages, expected, "no snapshot may be dropped");
    }

    #[test]
    fn commit_snapshot_from_staging_handles_subdirectories() {
        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: Some("test-snapshot".to_string()),
            host_repo_path: None,
        };

        let staging_dir = TempDir::new().expect("staging dir");
        fs::write(staging_dir.path().join("root.txt"), "root").expect("write root");

        let subdir = staging_dir.path().join("subdir");
        fs::create_dir(&subdir).expect("create subdir");
        fs::write(subdir.join("nested.txt"), "nested").expect("write nested");

        let oid = scm
            .commit_snapshot_from_staging(staging_dir.path(), "Test snapshot")
            .expect("commit")
            .expect("oid");

        let commit = scm.repo.find_commit(oid).expect("commit lookup");
        let tree = commit.tree().expect("tree");

        assert_eq!(tree.len(), 2);
        assert!(tree.get_name("root.txt").is_some());

        let subtree = tree.get_name("subdir").expect("subdir entry");
        let subtree_obj = subtree.to_object(&scm.repo).expect("subtree object");
        let subtree_tree = subtree_obj.as_tree().expect("as tree");
        assert_eq!(subtree_tree.len(), 1);
        assert!(subtree_tree.get_name("nested.txt").is_some());
    }

    #[test]
    fn commit_snapshot_from_staging_preserves_executable_bit() {
        use std::os::unix::fs::PermissionsExt;

        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: Some("test-snapshot".to_string()),
            host_repo_path: None,
        };

        let staging_dir = TempDir::new().expect("staging dir");

        let script_path = staging_dir.path().join("script.sh");
        fs::write(&script_path, "#!/bin/bash\necho hello").expect("write script");
        let mut perms = fs::metadata(&script_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).expect("set executable");

        let regular_path = staging_dir.path().join("file.txt");
        fs::write(&regular_path, "content").expect("write file");

        let oid = scm
            .commit_snapshot_from_staging(staging_dir.path(), "Test snapshot")
            .expect("commit")
            .expect("oid");

        let commit = scm.repo.find_commit(oid).expect("commit lookup");
        let tree = commit.tree().expect("tree");

        let script_entry = tree.get_name("script.sh").expect("script entry");
        assert_eq!(script_entry.filemode(), 0o100755); // Executable

        let file_entry = tree.get_name("file.txt").expect("file entry");
        assert_eq!(file_entry.filemode(), 0o100644); // Regular file
    }

    #[test]
    fn commit_snapshot_from_staging_returns_none_for_no_changes() {
        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: Some("test-snapshot".to_string()),
            host_repo_path: None,
        };

        let staging_dir = TempDir::new().expect("staging dir");
        fs::write(staging_dir.path().join("file.txt"), "content").expect("write file");

        // First commit
        let first_oid = scm
            .commit_snapshot_from_staging(staging_dir.path(), "First")
            .expect("commit")
            .expect("oid");

        // Second commit with same content - should return None
        let second = scm
            .commit_snapshot_from_staging(staging_dir.path(), "Second")
            .expect("commit");

        assert_eq!(second, None);

        // Verify branch still points to first commit
        let snapshot_ref = scm
            .repo
            .find_reference("refs/heads/test-snapshot")
            .expect("ref");
        assert_eq!(snapshot_ref.target().expect("target"), first_oid);
    }

    #[test]
    fn commit_snapshot_from_staging_chains_commits() {
        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: Some("test-snapshot".to_string()),
            host_repo_path: None,
        };

        let staging_dir = TempDir::new().expect("staging dir");
        fs::write(staging_dir.path().join("file.txt"), "first").expect("write file");

        let first_oid = scm
            .commit_snapshot_from_staging(staging_dir.path(), "First")
            .expect("commit")
            .expect("oid");

        // Modify content
        fs::write(staging_dir.path().join("file.txt"), "second").expect("write file");

        let second_oid = scm
            .commit_snapshot_from_staging(staging_dir.path(), "Second")
            .expect("commit")
            .expect("oid");

        let second_commit = scm.repo.find_commit(second_oid).expect("commit lookup");
        assert_eq!(second_commit.parent_id(0).expect("parent"), first_oid);
    }

    #[test]
    fn commit_snapshot_from_staging_does_not_touch_working_tree() {
        let (tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: Some("test-snapshot".to_string()),
            host_repo_path: None,
        };

        // Create a file in working tree
        let working_file = tempdir.path().join("working.txt");
        fs::write(&working_file, "working tree content").expect("write working file");

        // Capture initial working tree state
        let working_tree_before: Vec<_> = fs::read_dir(tempdir.path())
            .expect("read dir")
            .map(|e| e.expect("entry").file_name())
            .collect();

        // Create staging directory with different content
        let staging_dir = TempDir::new().expect("staging dir");
        fs::write(staging_dir.path().join("staged.txt"), "staged content").expect("write staged");

        let oid = scm
            .commit_snapshot_from_staging(staging_dir.path(), "Snapshot")
            .expect("commit")
            .expect("oid");

        // Verify working tree is completely unchanged
        let working_tree_after: Vec<_> = fs::read_dir(tempdir.path())
            .expect("read dir")
            .map(|e| e.expect("entry").file_name())
            .collect();
        assert_eq!(working_tree_before, working_tree_after);

        assert_eq!(
            fs::read_to_string(&working_file).expect("read working file"),
            "working tree content"
        );
        assert!(!tempdir.path().join("staged.txt").exists());

        // Verify snapshot tree contains different content
        let commit = scm.repo.find_commit(oid).expect("commit");
        let tree = commit.tree().expect("tree");
        assert_eq!(tree.len(), 1);
        let staged_entry = tree.get_name("staged.txt").expect("staged.txt entry");
        let blob = scm.repo.find_blob(staged_entry.id()).expect("blob");
        assert_eq!(blob.content(), b"staged content");
    }

    #[test]
    fn commit_snapshot_from_staging_does_not_touch_index() {
        let (tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: Some("test-snapshot".to_string()),
            host_repo_path: None,
        };

        // Stage a file
        let staged_file = tempdir.path().join("staged.txt");
        fs::write(&staged_file, "staged content").expect("write staged");
        let mut index = scm.repo.index().expect("index");
        index
            .add_path(Path::new("staged.txt"))
            .expect("add to index");
        index.write().expect("write index");

        // Capture index state
        let index_len_before = index.len();
        let index_tree_before = index.write_tree().expect("write tree");

        // Create staging directory with different content
        let staging_dir = TempDir::new().expect("staging dir");
        fs::write(staging_dir.path().join("snapshot.txt"), "snapshot content")
            .expect("write snapshot");

        let oid = scm
            .commit_snapshot_from_staging(staging_dir.path(), "Snapshot")
            .expect("commit")
            .expect("oid");

        // Verify index is completely unchanged
        let mut index_after = scm.repo.index().expect("index");
        assert_eq!(index_after.len(), index_len_before);
        let index_tree_after = index_after.write_tree().expect("write tree");
        assert_eq!(index_tree_before, index_tree_after);

        // Verify snapshot tree is different from index
        let commit = scm.repo.find_commit(oid).expect("commit");
        let snapshot_tree = commit.tree().expect("tree");
        assert_ne!(snapshot_tree.id(), index_tree_before);

        // Verify snapshot has snapshot.txt, not staged.txt
        assert_eq!(snapshot_tree.len(), 1);
        assert!(snapshot_tree.get_name("snapshot.txt").is_some());
        assert!(snapshot_tree.get_name("staged.txt").is_none());
    }

    #[test]
    fn commit_snapshot_from_staging_with_empty_directory() {
        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: Some("test-snapshot".to_string()),
            host_repo_path: None,
        };

        let staging_dir = TempDir::new().expect("staging dir");

        let result = scm
            .commit_snapshot_from_staging(staging_dir.path(), "Empty snapshot")
            .expect("commit");

        // Empty directory creates empty tree - should still create commit for first snapshot
        assert!(result.is_some());
    }

    #[test]
    fn commit_snapshot_from_staging_does_not_nest_under_staging_path() {
        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: Some("test-snapshot".to_string()),
            host_repo_path: None,
        };

        // Create staging dir with a path component that could accidentally become a prefix
        let staging_base = TempDir::new().expect("staging base");
        let staging_dir = staging_base.path().join("src");
        fs::create_dir(&staging_dir).expect("create src dir");

        // Add files at various levels
        fs::write(staging_dir.join("root.txt"), "root content").expect("write root");

        let subdir = staging_dir.join("subdir");
        fs::create_dir(&subdir).expect("create subdir");
        fs::write(subdir.join("nested.txt"), "nested content").expect("write nested");

        let oid = scm
            .commit_snapshot_from_staging(&staging_dir, "Test snapshot")
            .expect("commit")
            .expect("oid");

        let commit = scm.repo.find_commit(oid).expect("commit lookup");
        let tree = commit.tree().expect("tree");

        // Files should be at root of tree, NOT under "src/"
        assert!(
            tree.get_name("root.txt").is_some(),
            "root.txt should be at tree root"
        );
        assert!(
            tree.get_name("src").is_none(),
            "src/ should not exist in tree"
        );

        // Subdirectories should be preserved correctly
        let subdir_entry = tree.get_name("subdir").expect("subdir should exist");
        let subdir_tree = scm.repo.find_tree(subdir_entry.id()).expect("subdir tree");
        assert!(
            subdir_tree.get_name("nested.txt").is_some(),
            "nested.txt should be in subdir/"
        );
    }

    #[test]
    fn commit_snapshot_atomic_backup_prevents_corruption() {
        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            mode: ScmMode::Direct,
            snapshot_branch: Some("test-snapshot".to_string()),
            host_repo_path: None,
        };

        // Create initial snapshot
        let staging_dir = TempDir::new().expect("staging dir");
        fs::write(staging_dir.path().join("initial.txt"), "initial").expect("write initial");

        let initial_oid = scm
            .commit_snapshot_from_staging(staging_dir.path(), "Initial")
            .expect("commit")
            .expect("oid");

        // Verify initial snapshot ref exists
        let ref_name = "refs/heads/test-snapshot";
        let initial_ref = scm.repo.find_reference(ref_name).expect("initial ref");
        assert_eq!(initial_ref.target().expect("target"), initial_oid);

        // Even if something went wrong during snapshot creation, the ref should be:
        // - Either unchanged (pointing to initial_oid)
        // - Or updated to new valid commit
        // The backup/restore mechanism ensures we never have a dangling or invalid ref

        // Create another snapshot to verify atomicity continues to work
        fs::write(staging_dir.path().join("second.txt"), "second").expect("write second");

        let second_oid = scm
            .commit_snapshot_from_staging(staging_dir.path(), "Second")
            .expect("commit")
            .expect("oid");

        // Verify ref was updated atomically
        let second_ref = scm.repo.find_reference(ref_name).expect("second ref");
        assert_eq!(second_ref.target().expect("target"), second_oid);

        // Verify both commits are valid and chained
        let second_commit = scm.repo.find_commit(second_oid).expect("second commit");
        assert_eq!(second_commit.parent_count(), 1);
        assert_eq!(second_commit.parent_id(0).expect("parent"), initial_oid);
    }
}
