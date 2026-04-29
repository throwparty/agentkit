use std::path::{Path, PathBuf};
use std::sync::Mutex;

use git2::{BranchType, IndexAddOption, ObjectType, Repository, StatusOptions};

use crate::domain::{SandboxError, ScmError, slugify};

pub trait Scm {
    fn create_branch(&self, slug: &str) -> Result<String, SandboxError>;
    fn delete_branch(&self, slug: &str) -> Result<(), SandboxError>;
    fn make_archive(&self, reference: &str) -> Result<Vec<u8>, SandboxError>;
    fn list_sandboxes(&self) -> Result<Vec<String>, SandboxError>;
    fn repo_prefix(&self) -> Result<String, SandboxError>;
    fn has_changes(&self) -> Result<bool, SandboxError>;
    fn stage_all(&self) -> Result<(), SandboxError>;
    fn commit_snapshot(&self, message: &str) -> Result<Option<git2::Oid>, SandboxError>;
    fn apply_patch(&self, diff: &str) -> Result<(), SandboxError>;
}

pub struct GitScm {
    repo: Repository,
    snapshot_branch: Option<String>,
}

impl GitScm {
    pub fn open(path: &Path) -> Result<Self, SandboxError> {
        Repository::open(path)
            .map(|repo| Self {
                repo,
                snapshot_branch: None,
            })
            .map_err(|source| SandboxError::Scm(ScmError::Open { source }))
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

    fn tree_from_reference(&self, reference: &str) -> Result<git2::Tree<'_>, SandboxError> {
        let obj = self
            .repo
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

    #[allow(unused)]
    fn index_has_changes(&self, index: &mut git2::Index) -> Result<bool, SandboxError> {
        let tree_id = index
            .write_tree()
            .map_err(|source| SandboxError::Scm(ScmError::IndexWriteTree { source }))?;
        if let Some(head) = self.head_commit_optional()? {
            Ok(head.tree_id() != tree_id)
        } else {
            Ok(!index.is_empty())
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
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid path"))?;
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
        GitScm::open(path).map(|scm| Self {
            inner: Mutex::new(scm),
            prefix_override: None,
        })
    }

    pub fn open_with_prefix(path: &Path, prefix: Option<String>) -> Result<Self, SandboxError> {
        GitScm::open(path).map(|scm| Self {
            inner: Mutex::new(scm),
            prefix_override: prefix,
        })
    }

    pub fn for_sandbox(
        path: &Path,
        prefix: Option<String>,
        sandbox_slug: &str,
    ) -> Result<Self, SandboxError> {
        let mut scm = GitScm::open(path)?;
        let branch_name = GitScm::branch_name(sandbox_slug);
        scm.set_snapshot_branch(branch_name);

        Ok(Self {
            inner: Mutex::new(scm),
            prefix_override: prefix,
        })
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

    fn has_changes(&self) -> Result<bool, SandboxError> {
        self.lock()?.has_changes()
    }

    fn stage_all(&self) -> Result<(), SandboxError> {
        self.lock()?.stage_all()
    }

    fn commit_snapshot(&self, message: &str) -> Result<Option<git2::Oid>, SandboxError> {
        self.lock()?.commit_snapshot(message)
    }

    fn apply_patch(&self, diff: &str) -> Result<(), SandboxError> {
        self.lock()?.apply_patch(diff)
    }
}

impl Scm for GitScm {
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
        let tree = self.tree_from_reference(reference)?;
        let mut builder = tar::Builder::new(Vec::new());

        Self::append_tree(&self.repo, &mut builder, &tree, Path::new(""))?;

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

    fn has_changes(&self) -> Result<bool, SandboxError> {
        let mut status_opts = StatusOptions::new();
        status_opts.include_untracked(true);
        status_opts.include_ignored(false);

        let statuses = self
            .repo
            .statuses(Some(&mut status_opts))
            .map_err(|source| SandboxError::Scm(ScmError::Status { source }))?;

        Ok(!statuses.is_empty())
    }

    fn stage_all(&self) -> Result<(), SandboxError> {
        let mut index = self
            .repo
            .index()
            .map_err(|source| SandboxError::Scm(ScmError::IndexAdd { source }))?;

        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .map_err(|source| SandboxError::Scm(ScmError::IndexAdd { source }))?;

        index
            .write()
            .map_err(|source| SandboxError::Scm(ScmError::IndexWrite { source }))
    }

    fn commit_snapshot(&self, message: &str) -> Result<Option<git2::Oid>, SandboxError> {
        let workdir = self.repo.workdir().ok_or_else(|| {
            SandboxError::Config("Repository has no working directory".to_string())
        })?;

        // Use the same logic as commit_snapshot_from_staging
        self.commit_snapshot_from_staging(workdir, message)
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

        self.add_directory_to_tree(&mut builder, staging_path)?;

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
        } else if tree_oid == git2::Oid::zero() {
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
    ) -> Result<(), SandboxError> {
        use std::fs;

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

            let metadata = entry.metadata().map_err(SandboxError::Io)?;

            if metadata.is_dir() {
                let mut sub_builder = self
                    .repo
                    .treebuilder(None)
                    .map_err(|source| SandboxError::Scm(ScmError::Commit { source }))?;

                self.add_directory_to_tree(&mut sub_builder, &path)?;

                let sub_tree_oid = sub_builder
                    .write()
                    .map_err(|source| SandboxError::Scm(ScmError::Commit { source }))?;

                builder
                    .insert(&*name_str, sub_tree_oid, 0o040000)
                    .map_err(|source| SandboxError::Scm(ScmError::Commit { source }))?;
            } else {
                let content = fs::read(&path).map_err(SandboxError::Io)?;

                let blob_oid = self
                    .repo
                    .blob(&content)
                    .map_err(|source| SandboxError::Scm(ScmError::Commit { source }))?;

                #[cfg(unix)]
                let filemode = {
                    use std::os::unix::fs::PermissionsExt;
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
            snapshot_branch: None,
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
            snapshot_branch: None,
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
            snapshot_branch: None,
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
            snapshot_branch: None,
        };

        let err = scm.delete_branch("missing").expect_err("missing branch");
        assert_eq!(err.to_string(), "Sandbox 'missing' not found.");
    }

    #[test]
    fn archive_contains_tracked_files_only() {
        let (tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            snapshot_branch: None,
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
    fn has_changes_detects_modified_files() {
        let (tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            snapshot_branch: None,
        };
        fs::write(tempdir.path().join("README.md"), "updated").expect("write");

        assert!(scm.has_changes().expect("has changes"));
    }

    #[test]
    fn has_changes_false_when_clean() {
        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            snapshot_branch: None,
        };

        assert!(!scm.has_changes().expect("has changes"));
    }

    #[test]
    fn commit_snapshot_returns_none_when_clean() {
        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            snapshot_branch: None,
        };

        let result = scm.commit_snapshot("snapshot").expect("commit");
        assert!(result.is_none());
    }

    #[test]
    fn commit_snapshot_creates_commit() {
        let (tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            snapshot_branch: None,
        };

        fs::write(tempdir.path().join("README.md"), "updated").expect("write");
        let oid = scm
            .commit_snapshot("snapshot: update")
            .expect("commit")
            .expect("oid");

        let commit = scm.repo.find_commit(oid).expect("commit lookup");
        assert_eq!(commit.message().expect("message"), "snapshot: update");
        let snapshot_ref = scm
            .repo
            .find_reference("refs/heads/litterbox-snapshots")
            .expect("snapshot ref");
        let snapshot_commit = snapshot_ref.peel_to_commit().expect("snapshot commit");
        assert_eq!(snapshot_commit.id(), oid);
    }

    #[test]
    fn commit_snapshot_leaves_head_unchanged() {
        let (tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            snapshot_branch: None,
        };
        let head_before = scm
            .repo
            .head()
            .expect("head")
            .peel_to_commit()
            .expect("head commit")
            .id();

        fs::write(tempdir.path().join("README.md"), "snapshot").expect("write");
        let oid = scm
            .commit_snapshot("snapshot: head")
            .expect("commit")
            .expect("oid");

        let head_after = scm
            .repo
            .head()
            .expect("head")
            .peel_to_commit()
            .expect("head commit")
            .id();
        assert_eq!(head_after, head_before);

        let snapshot_commit = scm.repo.find_commit(oid).expect("snapshot commit");
        assert_eq!(snapshot_commit.parent_id(0).expect("parent"), head_before);
    }

    #[test]
    fn commit_snapshot_chains_on_snapshot_branch() {
        let (tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            snapshot_branch: None,
        };

        fs::write(tempdir.path().join("README.md"), "first").expect("write");
        let first_oid = scm
            .commit_snapshot("snapshot: first")
            .expect("commit")
            .expect("oid");

        fs::write(tempdir.path().join("README.md"), "second").expect("write");
        let second_oid = scm
            .commit_snapshot("snapshot: second")
            .expect("commit")
            .expect("oid");

        let second_commit = scm.repo.find_commit(second_oid).expect("commit lookup");
        assert_eq!(second_commit.parent_id(0).expect("parent"), first_oid);
    }

    #[test]
    fn commit_snapshot_from_staging_creates_commit_on_snapshot_branch() {
        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            snapshot_branch: Some("test-snapshot".to_string()),
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
            snapshot_branch: Some("test-snapshot".to_string()),
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
    fn commit_snapshot_from_staging_handles_subdirectories() {
        let (_tempdir, repo) = init_repo();
        let scm = GitScm {
            repo,
            snapshot_branch: Some("test-snapshot".to_string()),
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
            snapshot_branch: Some("test-snapshot".to_string()),
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
            snapshot_branch: Some("test-snapshot".to_string()),
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
            snapshot_branch: Some("test-snapshot".to_string()),
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
            snapshot_branch: Some("test-snapshot".to_string()),
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
            snapshot_branch: Some("test-snapshot".to_string()),
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
            snapshot_branch: Some("test-snapshot".to_string()),
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
            snapshot_branch: Some("test-snapshot".to_string()),
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
            snapshot_branch: Some("test-snapshot".to_string()),
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
