use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use git2::{BranchType, ObjectType, Repository};

use crate::domain::{SandboxError, ScmError, ScmMode, slugify};

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
    pub fn open(path: &Path, mode: ScmMode) -> Result<Self, SandboxError> {
        Repository::open(path)
            .map(|repo| Self {
                repo,
                mode,
                snapshot_branch: None,
                host_repo_path: None,
            })
            .map_err(|source| SandboxError::Scm(ScmError::Open { source }))
    }

    pub fn open_with_host(
        path: &Path,
        mode: ScmMode,
        host_repo_path: Option<PathBuf>,
    ) -> Result<Self, SandboxError> {
        Repository::open(path)
            .map(|repo| Self {
                repo,
                mode,
                snapshot_branch: None,
                host_repo_path,
            })
            .map_err(|source| SandboxError::Scm(ScmError::Open { source }))
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

    fn branch_name(slug: &str) -> String {
        format!("litterbox/{}", slug)
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
            .map_err(|source| SandboxError::Scm(ScmError::BranchCreate { source }))?;
        head.peel_to_commit()
            .map_err(|source| SandboxError::Scm(ScmError::BranchCreate { source }))
    }

    fn tree_from_reference_in<'a>(
        repo: &'a Repository,
        reference: &str,
    ) -> Result<git2::Tree<'a>, SandboxError> {
        let obj = repo
            .revparse_single(reference)
            .map_err(|source| SandboxError::Scm(ScmError::Archive { source }))?;
        obj.peel_to_tree()
            .map_err(|source| SandboxError::Scm(ScmError::Archive { source }))
    }

    fn signature(&self) -> Result<git2::Signature<'_>, SandboxError> {
        self.repo
            .signature()
            .or_else(|_| git2::Signature::now("Litterbox", "noreply@example.com"))
            .map_err(|source| SandboxError::Scm(ScmError::Signature { source }))
    }

    fn head_commit_optional(&self) -> Result<Option<git2::Commit<'_>>, SandboxError> {
        match self.repo.head() {
            Ok(head) => head
                .peel_to_commit()
                .map(Some)
                .map_err(|source| SandboxError::Scm(ScmError::Head { source })),
            Err(error) if error.code() == git2::ErrorCode::UnbornBranch => Ok(None),
            Err(source) => Err(SandboxError::Scm(ScmError::Head { source })),
        }
    }

    fn snapshot_branch_ref(&self) -> String {
        match &self.snapshot_branch {
            Some(branch) => format!("refs/heads/{}", branch),
            None => "refs/heads/litterbox-snapshots".to_string(),
        }
    }

    fn snapshot_parent(&self) -> Result<Option<git2::Commit<'_>>, SandboxError> {
        match self.repo.find_reference(&self.snapshot_branch_ref()) {
            Ok(reference) => reference
                .peel_to_commit()
                .map(Some)
                .map_err(|source| SandboxError::Scm(ScmError::Reference { source })),
            Err(error) if error.code() == git2::ErrorCode::NotFound => self.head_commit_optional(),
            Err(source) => Err(SandboxError::Scm(ScmError::Reference { source })),
        }
    }

    fn append_tree(
        repo: &Repository,
        builder: &mut tar::Builder<Vec<u8>>,
        tree: &git2::Tree<'_>,
        base: &Path,
    ) -> Result<(), SandboxError> {
        for entry in tree.iter() {
            Self::append_entry(repo, builder, base, &entry)?;
        }

        Ok(())
    }

    fn append_entry(
        repo: &Repository,
        builder: &mut tar::Builder<Vec<u8>>,
        base: &Path,
        entry: &git2::TreeEntry<'_>,
    ) -> Result<(), SandboxError> {
        let name = entry
            .name()
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid path"))?;
        let path = base.join(PathBuf::from(name));

        match entry.kind() {
            Some(ObjectType::Tree) => {
                let subtree = entry
                    .to_object(repo)
                    .map_err(|source| SandboxError::Scm(ScmError::Archive { source }))?
                    .peel_to_tree()
                    .map_err(|source| SandboxError::Scm(ScmError::Archive { source }))?;
                Self::append_tree(repo, builder, &subtree, &path)
            }
            Some(ObjectType::Blob) => Self::append_blob(repo, builder, &path, entry),
            _ => Ok(()),
        }
    }

    fn append_blob(
        repo: &Repository,
        builder: &mut tar::Builder<Vec<u8>>,
        path: &Path,
        entry: &git2::TreeEntry<'_>,
    ) -> Result<(), SandboxError> {
        let blob = entry
            .to_object(repo)
            .map_err(|source| SandboxError::Scm(ScmError::Archive { source }))?
            .peel_to_blob()
            .map_err(|source| SandboxError::Scm(ScmError::Archive { source }))?;

        let mut header = tar::Header::new_gnu();
        let mode = match entry.filemode() {
            0 => 0o644,
            value => value as u32,
        };
        let size = u64::try_from(blob.size())
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "blob too large"))?;
        header.set_size(size);
        header.set_mode(mode);
        header.set_cksum();

        builder.append_data(&mut header, path, blob.content())?;
        Ok(())
    }
}

pub struct ThreadSafeScm {
    inner: Mutex<GitScm>,
    prefix_override: Option<String>,
}

impl ThreadSafeScm {
    pub fn open(path: &Path) -> Result<Self, SandboxError> {
        GitScm::open_direct(path).map(|scm| Self {
            inner: Mutex::new(scm),
            prefix_override: None,
        })
    }

    pub fn open_with_mode(path: &Path, mode: ScmMode) -> Result<Self, SandboxError> {
        GitScm::open(path, mode).map(|scm| Self {
            inner: Mutex::new(scm),
            prefix_override: None,
        })
    }

    pub fn open_with_prefix(path: &Path, prefix: Option<String>) -> Result<Self, SandboxError> {
        GitScm::open_direct(path).map(|scm| Self {
            inner: Mutex::new(scm),
            prefix_override: prefix,
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
        })
    }

    pub fn set_snapshot_branch(&self, branch: String) -> Result<(), SandboxError> {
        self.lock()?.set_snapshot_branch(branch);
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

impl Scm for ThreadSafeScm {
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
            .map_err(|source| SandboxError::Scm(ScmError::BranchCreate { source }))?;

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
            .map_err(|source| SandboxError::Scm(ScmError::BranchDelete { source }))
    }

    fn make_archive(&self, reference: &str) -> Result<Vec<u8>, SandboxError> {
        let repo = match self.mode {
            ScmMode::Remote => {
                if let Some(ref host_path) = self.host_repo_path {
                    &Repository::open(host_path)
                        .map_err(|source| SandboxError::Scm(ScmError::Open { source }))?
                } else {
                    &self.repo
                }
            }
            ScmMode::Direct => &self.repo,
        };
        let tree = Self::tree_from_reference_in(repo, reference)?;
        let mut builder = tar::Builder::new(Vec::new());

        Self::append_tree(repo, &mut builder, &tree, Path::new(""))?;

        builder.into_inner().map_err(SandboxError::Io)
    }

    fn list_sandboxes(&self) -> Result<Vec<String>, SandboxError> {
        let mut sandboxes = Vec::new();
        let branches = self
            .repo
            .branches(Some(BranchType::Local))
            .map_err(|source| SandboxError::Scm(ScmError::BranchList { source }))?;

        for branch in branches {
            let (branch, _) =
                branch.map_err(|source| SandboxError::Scm(ScmError::BranchList { source }))?;
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
            SandboxError::Scm(ScmError::ApplyPatch {
                message: format!("Failed to parse diff: {}", e),
            })
        })?;

        self.repo
            .apply(&diff_obj, git2::ApplyLocation::WorkDir, None)
            .map_err(|e| {
                SandboxError::Scm(ScmError::ApplyPatch {
                    message: format!("Failed to apply patch: {}", e),
                })
            })
    }
}

impl GitScm {
    /// The paths the snapshot's parent records, which stay tracked however the
    /// sandbox's ignore rules read.
    fn snapshot_tracked(&self) -> Result<(HashSet<PathBuf>, HashSet<PathBuf>), SandboxError> {
        let mut tracked = HashSet::new();
        let mut tracked_dirs = HashSet::new();

        let Some(parent) = self.snapshot_parent()? else {
            return Ok((tracked, tracked_dirs));
        };
        let tree = parent
            .tree()
            .map_err(|source| SandboxError::Scm(ScmError::Commit { source }))?;

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
        .map_err(|source| SandboxError::Scm(ScmError::Commit { source }))?;

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

    fn commit_snapshot_from_staging(
        &self,
        staging_path: &Path,
        message: &str,
    ) -> Result<Option<git2::Oid>, SandboxError> {
        let parent = self.snapshot_parent()?;
        let signature = self.signature()?;

        // Backup snapshot branch ref before modification (for atomic recovery)
        let backup = self.backup_snapshot_ref()?;

        // Build a new tree from staging directory
        let mut builder = self
            .repo
            .treebuilder(None)
            .map_err(|source| SandboxError::Scm(ScmError::Commit { source }))?;

        // The staged tree is the sandbox's, so its rules are the ones that
        // decide what the snapshot records; the transfer applied the same ones.
        let filter = self.snapshot_filter(staging_path)?;
        self.add_directory_to_tree(&mut builder, staging_path, Path::new(""), &filter)?;

        let tree_oid = builder.write().map_err(|e| {
            // Restore backup on failure
            let _ = self.restore_snapshot_ref(&backup);
            SandboxError::Scm(ScmError::Commit { source: e })
        })?;

        // Check if tree changed
        if let Some(ref parent_commit) = parent {
            if parent_commit.tree_id() == tree_oid {
                return Ok(None);
            }
        } else if tree_oid == git2::Oid::ZERO_SHA1 {
            return Ok(None);
        }

        let tree = self.repo.find_tree(tree_oid).map_err(|e| {
            let _ = self.restore_snapshot_ref(&backup);
            SandboxError::Scm(ScmError::Commit { source: e })
        })?;

        let parents: Vec<&git2::Commit> = parent.as_ref().map(|c| vec![c]).unwrap_or_default();

        // Create commit without updating ref (to avoid "current tip is not first parent" when jj modifies branch)
        let oid = self
            .repo
            .commit(
                None, // Don't update ref yet
                &signature, &signature, message, &tree, &parents,
            )
            .map_err(|e| {
                let _ = self.restore_snapshot_ref(&backup);
                SandboxError::Scm(ScmError::Commit { source: e })
            })?;

        // Force update the ref to point to our new commit (handles concurrent jj updates)
        // Retry with backoff if locked (jj may be holding the lock)
        let mut retries = 0;
        let max_retries = 5;
        loop {
            let result = match self.repo.find_reference(&self.snapshot_branch_ref()) {
                Ok(mut reference) => reference.set_target(oid, message),
                Err(e) if e.code() == git2::ErrorCode::NotFound => {
                    self.repo
                        .reference(&self.snapshot_branch_ref(), oid, false, message)
                }
                Err(e) => Err(e),
            };

            match result {
                Ok(_) => break,
                Err(e) if e.code() == git2::ErrorCode::Locked && retries < max_retries => {
                    retries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(10 * retries as u64));
                    continue;
                }
                Err(e) => {
                    let _ = self.restore_snapshot_ref(&backup);
                    return Err(SandboxError::Scm(ScmError::Commit { source: e }));
                }
            }
        }

        Ok(Some(oid))
    }

    fn backup_snapshot_ref(&self) -> Result<Option<git2::Oid>, SandboxError> {
        let ref_name = self.snapshot_branch_ref();
        match self.repo.find_reference(&ref_name) {
            Ok(reference) => Ok(reference.target()),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(source) => Err(SandboxError::Scm(ScmError::Commit { source })),
        }
    }

    fn restore_snapshot_ref(&self, backup: &Option<git2::Oid>) -> Result<(), SandboxError> {
        let ref_name = self.snapshot_branch_ref();

        match backup {
            Some(oid) => {
                // Restore to previous oid
                self.repo
                    .reference(&ref_name, *oid, true, "Restore from backup")
                    .map_err(|source| SandboxError::Scm(ScmError::Commit { source }))?;
            }
            None => {
                // Ref didn't exist before, delete it
                match self.repo.find_reference(&ref_name) {
                    Ok(mut reference) => {
                        reference
                            .delete()
                            .map_err(|source| SandboxError::Scm(ScmError::Commit { source }))?;
                    }
                    Err(e) if e.code() == git2::ErrorCode::NotFound => {
                        // Already gone, nothing to do
                    }
                    Err(source) => {
                        return Err(SandboxError::Scm(ScmError::Commit { source }));
                    }
                }
            }
        }

        Ok(())
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
                    .map_err(|source| SandboxError::Scm(ScmError::Commit { source }))?;

                self.add_directory_to_tree(&mut sub_builder, &path, &child, filter)?;

                let sub_tree_oid = sub_builder
                    .write()
                    .map_err(|source| SandboxError::Scm(ScmError::Commit { source }))?;

                builder
                    .insert(&*name_str, sub_tree_oid, 0o040000)
                    .map_err(|source| SandboxError::Scm(ScmError::Commit { source }))?;
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
                    .map_err(|source| SandboxError::Scm(ScmError::Commit { source }))?;

                builder
                    .insert(&*name_str, blob_oid, 0o120000)
                    .map_err(|source| SandboxError::Scm(ScmError::Commit { source }))?;
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
                    .map_err(|source| SandboxError::Scm(ScmError::Commit { source }))?;
                std::io::copy(&mut source, &mut writer).map_err(SandboxError::Io)?;
                let blob_oid = writer
                    .commit()
                    .map_err(|source| SandboxError::Scm(ScmError::Commit { source }))?;

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
                    .map_err(|source| SandboxError::Scm(ScmError::Commit { source }))?;
            }
        }

        Ok(())
    }
}

/// What a snapshot tracks, decided the way `git add -A` decides it: a path the
/// snapshot's parent already records is tracked, and the sandbox's ignore rules
/// apply only to everything else.
pub struct SnapshotFilter {
    /// The checkout libgit2 evaluates rules against. Kept alive because it owns
    /// the directory the sandbox's rule files are mirrored into.
    _rules: tempfile::TempDir,
    checkout: Mutex<Repository>,
    tracked: HashSet<PathBuf>,
    tracked_dirs: HashSet<PathBuf>,
}

impl SnapshotFilter {
    fn open(
        rules_root: &Path,
        tracked: HashSet<PathBuf>,
        tracked_dirs: HashSet<PathBuf>,
    ) -> Result<Self, SandboxError> {
        let rules = tempfile::tempdir().map_err(SandboxError::Io)?;
        let checkout = Repository::init(rules.path())
            .map_err(|source| SandboxError::Scm(ScmError::Open { source }))?;

        // libgit2 answers ignore questions about a checkout, so the sandbox's
        // own ignore files are mirrored into this one at the same paths: a
        // nested rule keeps meaning what it means inside the sandbox.
        mirror_ignore_files(rules_root, rules.path())?;

        // A repository's own exclude file applies from its root.
        if let Ok(contents) = fs::read(rules_root.join(".git").join("info").join("exclude")) {
            let target = checkout.path().join("info").join("exclude");
            fs::write(target, contents).map_err(SandboxError::Io)?;
        }

        Ok(Self {
            _rules: rules,
            checkout: Mutex::new(checkout),
            tracked,
            tracked_dirs,
        })
    }

    fn skips_file(&self, relative: &Path) -> Result<bool, SandboxError> {
        if self.tracked.contains(relative) {
            return Ok(false);
        }
        self.ignored(relative)
    }

    /// A directory is skipped only when the snapshot's parent records nothing
    /// beneath it.
    fn skips_dir(&self, relative: &Path) -> Result<bool, SandboxError> {
        if self.tracked_dirs.contains(relative) {
            return Ok(false);
        }
        // The staging tree is not the checkout, so libgit2 cannot stat the
        // directory to learn it is one; the trailing separator says so.
        let mut probe = relative.as_os_str().to_owned();
        probe.push("/");
        self.ignored(Path::new(&probe))
    }

    fn ignored(&self, relative: &Path) -> Result<bool, SandboxError> {
        let checkout = self
            .checkout
            .lock()
            .map_err(|_| SandboxError::Config("Mutex poisoned".to_string()))?;
        checkout
            .status_should_ignore(relative)
            .map_err(|source| SandboxError::Scm(ScmError::Status { source }))
    }
}

/// Copy a tree's ignore files into a checkout, leaving everything else behind.
///
/// `from` is a tree the sandbox produced; `to` is the scratch checkout libgit2
/// reads rules from. Only the rule files are needed, so nothing else is copied
/// and no rule changes meaning: `.gitignore` governs the directory it sits in.
fn mirror_ignore_files(from: &Path, to: &Path) -> Result<(), SandboxError> {
    let entries = match fs::read_dir(from) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(SandboxError::Io(error)),
    };

    for entry in entries {
        let entry = entry.map_err(SandboxError::Io)?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        // `file_type` does not follow a link, so a linked directory is left
        // alone rather than walked in circles.
        if entry.file_type().map_err(SandboxError::Io)?.is_dir() {
            mirror_ignore_files(&entry.path(), &to.join(&name))?;
        } else if name == ".gitignore" {
            fs::create_dir_all(to).map_err(SandboxError::Io)?;
            fs::copy(entry.path(), to.join(&name)).map_err(SandboxError::Io)?;
        }
    }

    Ok(())
}

impl crate::sandbox::contract::SnapshotSelector for SnapshotFilter {
    fn selects(
        &self,
        relative: &Path,
        is_dir: bool,
    ) -> Result<bool, crate::sandbox::contract::Error> {
        let skips = if is_dir {
            self.skips_dir(relative)
        } else {
            self.skips_file(relative)
        };
        skips
            .map(|skips| !skips)
            .map_err(|error| crate::sandbox::contract::Error::failed(error.to_string()))
    }
}

/// Selects the ignore files a snapshot's rules are read from.
///
/// A snapshot has to know the sandbox's own rules before it decides what to
/// transfer, so the transfer runs twice: once for these files, and once for
/// everything else under the rules they describe.
pub struct IgnoreFiles;

impl crate::sandbox::contract::SnapshotSelector for IgnoreFiles {
    fn selects(
        &self,
        relative: &Path,
        is_dir: bool,
    ) -> Result<bool, crate::sandbox::contract::Error> {
        // A directory is kept so the walk reaches the rules nested inside it.
        if is_dir {
            return Ok(true);
        }
        Ok(relative
            .file_name()
            .is_some_and(|name| name == ".gitignore")
            || relative == Path::new(".git/info/exclude"))
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
            repo.commit(
                Some("HEAD"),
                &signature,
                &signature,
                "track kept",
                &tree,
                &[&parent],
            )
            .expect("commit kept");
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
        symlink(outside.path().join("secret"), staging_dir.path().join("link"))
            .expect("symlink");
        // A link back up the tree must not be followed, or the walk recurses.
        symlink("..", staging_dir.path().join("loop")).expect("loop symlink");

        let oid = scm
            .commit_snapshot_from_staging(staging_dir.path(), "Test snapshot")
            .expect("commit")
            .expect("oid");

        let tree = scm.repo.find_commit(oid).expect("commit").tree().expect("tree");
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
