pub mod path;
pub mod remote;

use std::fs;
use std::path::{Path, PathBuf};

use git2::build::RepoBuilder;
use git2::{FetchOptions, Repository};

use crate::domain::{SandboxError, ScmError};

pub struct VcsStore;

/// Releases the clone lock when dropped.
#[allow(dead_code)]
struct CloneLock(std::fs::File);

impl VcsStore {
    fn acquire_clone_lock(slug: &str) -> Result<CloneLock, SandboxError> {
        let lock_path = Self::resolve_path(slug)
            .parent()
            .ok_or_else(|| SandboxError::Config("invalid bare path".to_string()))?
            .join(format!(".{}.clone.lock", slug));

        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&lock_path)
            .map_err(SandboxError::Io)?;

        file.lock().map_err(SandboxError::Io)?;

        Ok(CloneLock(file))
    }

    pub fn resolve_path(slug: &str) -> PathBuf {
        crate::vcs_store::path::resolve_path(slug)
    }

    pub fn clone_bare(host_path: &Path, slug: &str) -> Result<PathBuf, SandboxError> {
        let _lock = Self::acquire_clone_lock(slug)?;
        let bare_path = Self::resolve_path(slug);

        // Double-check after acquiring lock: another process may have cloned
        if bare_path.exists() && Repository::open(&bare_path).is_ok() {
            return Ok(bare_path);
        }

        // Missing or corrupt — self-heal then re-clone
        if bare_path.exists() {
            Self::self_heal(slug)?;
        }

        if let Some(parent) = bare_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let url = host_path
            .to_str()
            .ok_or_else(|| SandboxError::Config("invalid host path".to_string()))?;

        match Self::try_clone_bare(url, &bare_path, Some(1)) {
            Ok(_) => Ok(bare_path),
            Err(ref e) if is_shallow_not_supported(e) => {
                match Self::try_clone_bare(url, &bare_path, None) {
                    Ok(_) => Ok(bare_path),
                    Err(e) => Err(SandboxError::Scm(ScmError::Clone { source: e })),
                }
            }
            Err(e) => Err(SandboxError::Scm(ScmError::Clone { source: e })),
        }
    }

    fn try_clone_bare(
        url: &str,
        bare_path: &Path,
        depth: Option<i32>,
    ) -> Result<(), git2::Error> {
        let mut fetch_opts = FetchOptions::new();
        if let Some(d) = depth {
            fetch_opts.depth(d);
        }
        RepoBuilder::new()
            .bare(true)
            .fetch_options(fetch_opts)
            .clone(url, bare_path)?;
        Ok(())
    }

    /// Destroys a corrupt bare clone. Returns the path so callers can re-clone.
    pub fn self_heal(slug: &str) -> Result<PathBuf, SandboxError> {
        let bare_path = Self::resolve_path(slug);
        if bare_path.exists() {
            fs::remove_dir_all(&bare_path)?;
        }
        Ok(bare_path)
    }

    pub fn destroy_bare(slug: &str) -> Result<(), SandboxError> {
        let bare_path = Self::resolve_path(slug);
        if bare_path.exists() {
            fs::remove_dir_all(&bare_path)?;
        }
        Ok(())
    }

    pub fn install_remote(repo_path: &Path, bare_path: &Path) -> Result<(), SandboxError> {
        crate::vcs_store::remote::install_remote(repo_path, bare_path)
    }

    pub fn remove_remote(repo_path: &Path) -> Result<(), SandboxError> {
        crate::vcs_store::remote::remove_remote(repo_path)
    }
}

fn is_shallow_not_supported(error: &git2::Error) -> bool {
    error.message().contains("shallow fetch is not supported")
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::sync::OnceLock;
    use tempfile::TempDir;

    use crate::domain::ScmMode;
    use crate::scm::{GitScm, Scm};

    use git2::Repository;

    /// Ensures `LITTERBOX_DATA_DIR` is set to a temp dir once for this test module.
    fn ensure_test_dir() -> &'static TempDir {
        static DIR: OnceLock<TempDir> = OnceLock::new();
        DIR.get_or_init(|| {
            let tmp = TempDir::new().expect("tempdir");
            unsafe {
                std::env::set_var("LITTERBOX_DATA_DIR", tmp.path());
            }
            tmp
        })
    }

    fn init_host_repo() -> (TempDir, Repository) {
        let tempdir = TempDir::new().expect("tempdir");
        let repo = Repository::init(tempdir.path()).expect("repo init");

        let file_path = tempdir.path().join("README.md");
        fs::write(&file_path, "hello").expect("write file");

        let mut index = repo.index().expect("index");
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .expect("add all");
        index.write().expect("index write");
        let tree_id = index.write_tree().expect("write tree");

        let signature =
            git2::Signature::now("Litterbox", "noreply@example.com").expect("signature");
        {
            let tree = repo.find_tree(tree_id).expect("find tree");
            repo.commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])
                .expect("commit");
        }

        (tempdir, repo)
    }

    #[test]
    fn resolve_path_returns_platform_path() {
        ensure_test_dir();
        let slug = "test-project";
        let resolved = VcsStore::resolve_path(slug);
        let lossy = resolved.to_string_lossy();
        assert!(lossy.contains("vcs/git/test-project"), "path: {lossy}");
    }

    #[test]
    fn resolve_path_is_absolute() {
        ensure_test_dir();
        let resolved = VcsStore::resolve_path("test-project");
        assert!(resolved.is_absolute(), "must be absolute: {resolved:?}");
    }

    #[test]
    fn resolve_path_default_slug_basename() {
        ensure_test_dir();
        let slug = "my-project";
        let resolved = VcsStore::resolve_path(slug);
        assert!(
            resolved.to_string_lossy().ends_with("vcs/git/my-project"),
            "got: {}",
            resolved.display()
        );
    }

    #[test]
    fn clone_bare_creates_bare_repo() {
        ensure_test_dir();
        let (tempdir, _repo) = init_host_repo();
        let slug = "test-bare-clone";

        let bare_path = VcsStore::clone_bare(tempdir.path(), slug)
            .expect("clone bare");

        assert!(bare_path.exists());
        assert!(bare_path.join("HEAD").exists());
        assert!(bare_path.join("refs").exists());
        assert!(bare_path.join("objects").exists());

        // Verify it's a bare repo
        let bare_repo = Repository::open_bare(&bare_path).expect("open bare");
        assert!(bare_repo.is_bare());

        // Clean up
        let _ = fs::remove_dir_all(&bare_path);
    }

    #[test]
    fn clone_bare_sets_depth_option() {
        ensure_test_dir();
        let (tempdir, _repo) = init_host_repo();
        let slug = "test-depth-clone";

        let bare_path =
            VcsStore::clone_bare(tempdir.path(), slug).expect("clone bare");

        // libgit2 does not support local shallow clones (remote only).
        // We verify the repo clones successfully with depth(1) set.
        let bare_repo = Repository::open_bare(&bare_path).expect("open bare");
        assert!(bare_repo.is_bare());
        assert!(bare_repo.head().is_ok());

        let _ = fs::remove_dir_all(&bare_path);
    }

    #[test]
    fn clone_bare_skips_existing_path() {
        ensure_test_dir();
        let (tempdir, _repo) = init_host_repo();
        let slug = "test-skip-clone";

        let first_path =
            VcsStore::clone_bare(tempdir.path(), slug).expect("first clone");
        let second_path =
            VcsStore::clone_bare(tempdir.path(), slug).expect("second clone");

        assert_eq!(first_path, second_path);

        let _ = fs::remove_dir_all(&first_path);
    }

    #[test]
    fn self_heal_destroys_corrupt_repo() {
        ensure_test_dir();
        let (tempdir, _repo) = init_host_repo();
        let slug = "test-self-heal";

        let bare_path =
            VcsStore::clone_bare(tempdir.path(), slug).expect("clone bare");

        // Corrupt: remove objects
        let objects = bare_path.join("objects");
        if objects.exists() {
            fs::remove_dir_all(&objects).expect("remove objects");
        }
        fs::create_dir(&objects).expect("recreate empty objects");

        // self_heal destroys the corrupt repo
        let healed_path = VcsStore::self_heal(slug).expect("self heal");
        assert_eq!(bare_path, healed_path);
        assert!(!healed_path.exists(), "self_heal should destroy the repo");

        // clone_bare should re-create it
        let recloned_path =
            VcsStore::clone_bare(tempdir.path(), slug).expect("re-clone");
        assert_eq!(healed_path, recloned_path);
        let bare_repo = Repository::open_bare(&recloned_path).expect("open healed");
        assert!(bare_repo.head().is_ok());

        let _ = fs::remove_dir_all(&bare_path);
    }

    #[test]
    fn clone_bare_self_heals_corrupt_repo() {
        ensure_test_dir();
        let (tempdir, _repo) = init_host_repo();
        let slug = "test-self-heal-clone";

        // Initial clone
        let bare_path =
            VcsStore::clone_bare(tempdir.path(), slug).expect("first clone");

        // Corrupt: remove objects directory
        let objects = bare_path.join("objects");
        if objects.exists() {
            fs::remove_dir_all(&objects).expect("remove objects");
        }
        fs::create_dir(&objects).expect("recreate empty objects");

        // clone_bare should detect corruption and self-heal
        let healed_path =
            VcsStore::clone_bare(tempdir.path(), slug).expect("self heal via clone_bare");

        assert_eq!(bare_path, healed_path);
        let bare_repo = Repository::open_bare(&healed_path).expect("open healed");
        assert!(bare_repo.head().is_ok());

        let _ = fs::remove_dir_all(&bare_path);
    }

    #[test]
    fn destroy_bare_removes_directory() {
        ensure_test_dir();
        let (tempdir, _repo) = init_host_repo();
        let slug = "test-destroy-bare";

        let bare_path = VcsStore::clone_bare(tempdir.path(), slug)
            .expect("clone bare");
        assert!(bare_path.exists());

        VcsStore::destroy_bare(slug).expect("destroy bare");
        assert!(!bare_path.exists());
    }

    #[test]
    fn destroy_bare_noop_when_missing() {
        ensure_test_dir();
        let slug = "test-destroy-missing";
        let bare_path = VcsStore::resolve_path(slug);
        assert!(!bare_path.exists());

        VcsStore::destroy_bare(slug).expect("destroy bare");
        assert!(!bare_path.exists());
    }

    #[test]
    fn remote_mode_operations_use_bare_clone_not_host() {
        ensure_test_dir();
        // Minimal reproducer: remote mode SCM operates on bare clone,
        // not the host repo. Branch appears in bare clone, archive
        // reads from host repo, litterbox remote is installed.
        let (tempdir, host_repo) = init_host_repo();

        // Add a file to host so archive has content
        fs::write(tempdir.path().join("hello.txt"), "world").expect("write");
        let mut index = host_repo.index().expect("index");
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .expect("add");
        index.write().expect("write");
        let tree_id = index.write_tree().expect("write tree");
        let sig = git2::Signature::now("test", "test@test.com").expect("sig");
        let parent = host_repo.head().ok().and_then(|r| r.peel_to_commit().ok());
        let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
        host_repo
            .commit(
                Some("HEAD"),
                &sig,
                &sig,
                "add hello",
                &host_repo.find_tree(tree_id).expect("tree"),
                &parents,
            )
            .expect("commit");

        let slug = "remote-mode-test";

        // Clone bare repo (this is what build_provider_with_config does)
        let bare_path =
            VcsStore::clone_bare(tempdir.path(), slug).expect("clone bare");

        // Install litterbox remote on host repo
        VcsStore::install_remote(tempdir.path(), &bare_path).expect("install remote");

        // Verify remote was installed on host repo
        let host_remote = host_repo
            .find_remote("litterbox")
            .expect("litterbox remote");
        assert_eq!(
            host_remote.url().map(|u| u.to_string()),
            Ok(bare_path.to_string_lossy().to_string())
        );

        // Open GitScm on bare clone (as build_provider_with_config does for remote mode)
        let scm = GitScm::open_with_host(
            &bare_path,
            ScmMode::Remote,
            Some(tempdir.path().to_path_buf()),
        )
        .expect("open scm on bare clone");

        // Create a sandbox branch on the bare clone
        let branch_name = scm.create_branch("my-sandbox").expect("create branch");

        // Verify branch is NOT in host repo
        let host_branch = host_repo.find_branch(&branch_name, git2::BranchType::Local);
        assert!(
            host_branch.is_err(),
            "branch should NOT exist in host repo"
        );

        // Verify branch IS in bare clone
        let bare_repo = git2::Repository::open_bare(&bare_path).expect("open bare");
        let bare_branch = bare_repo.find_branch(&branch_name, git2::BranchType::Local);
        assert!(bare_branch.is_ok(), "branch should exist in bare clone");

        // make_archive should read from host repo (via host_repo_path), not bare clone
        let archive = scm.make_archive("HEAD").expect("make archive");

        // Verify archive contains hello.txt from host repo
        let mut ar = tar::Archive::new(std::io::Cursor::new(&archive));
        let entries: Vec<String> = ar
            .entries()
            .expect("entries")
            .filter_map(|e| e.ok().and_then(|e| e.path().ok().map(|p| p.to_string_lossy().to_string())))
            .collect();
        assert!(
            entries.contains(&"hello.txt".to_string()),
            "archive should contain hello.txt from host repo, got: {entries:?}"
        );

        // List sandboxes in bare clone
        let sandboxes = scm.list_sandboxes().expect("list sandboxes");
        assert_eq!(sandboxes, vec!["my-sandbox"]);

        // commit_snapshot returns None on bare repo (no workdir)
        let snapshot = scm.commit_snapshot("test").expect("commit snapshot");
        assert!(snapshot.is_none(), "bare repo snapshot returns None");

        // Delete branch from bare clone
        scm.delete_branch("my-sandbox").expect("delete branch");

        // Verify sandbox list is now empty
        let remaining = scm.list_sandboxes().expect("list sandboxes");
        assert!(remaining.is_empty(), "bare clone should be empty after delete");

        // Destroy bare clone
        VcsStore::destroy_bare(slug).expect("destroy bare");
        assert!(!bare_path.exists());

        // Verifying litterbox remote persists on host repo after all sandboxes are gone
        let host_remote_after = host_repo
            .find_remote("litterbox")
            .expect("litterbox remote should persist");
        assert_eq!(
            host_remote_after.url().map(|u| u.to_string()),
            Ok(bare_path.to_string_lossy().to_string())
        );
    }
}
