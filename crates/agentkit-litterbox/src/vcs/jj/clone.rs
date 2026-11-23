//! The co-located jj clone and the directories it owns.

use std::path::{Path, PathBuf};

use crate::vcs::Error;

/// The clone for one project.
pub fn root(slug: &str) -> PathBuf {
    agentkit_common::data_dir("litterbox")
        .join("vcs")
        .join("jj")
        .join(slug)
}

/// One sandbox's working copy.
pub fn workspace(root: &Path, sandbox: &str) -> PathBuf {
    root.join("workspaces").join(sandbox)
}

/// One sandbox's staging workspace.
#[allow(dead_code)] // wired into staging in T-009
pub fn reconcile(root: &Path, sandbox: &str) -> PathBuf {
    root.join("reconcile").join(sandbox)
}

/// Create the clone from `host` when it is absent, and return its root.
#[allow(dead_code)] // wired into prepare in T-008
pub fn ensure(root: &Path, host: &Path) -> Result<(), Error> {
    if root.join(".jj").is_dir() {
        return Ok(());
    }
    if let Some(parent) = root.parent() {
        std::fs::create_dir_all(parent).map_err(Error::Io)?;
    }
    let output = std::process::Command::new("jj")
        .args(["git", "clone"])
        .arg(host)
        .arg(root)
        .output()
        .map_err(|error| Error::Backend {
            operation: "clone".to_string(),
            message: format!("could not run jj: {error}"),
        })?;
    if !output.status.success() {
        return Err(Error::Backend {
            operation: "clone".to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(())
}

/// A held clone lock.
#[allow(dead_code)] // wired into the operations in T-008
pub struct Lock(std::fs::File);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Take the clone-level lock, so snapshot, bookmark moves and staging do not
/// interleave across the shared repository.
#[allow(dead_code)] // wired into the operations in T-008
pub fn lock(root: &Path) -> Result<Lock, Error> {
    std::fs::create_dir_all(root).map_err(Error::Io)?;
    let path = root.join(".litterbox.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(Error::Io)?;
    file.lock().map_err(Error::Io)?;
    Ok(Lock(file))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .args(args)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    }

    fn repository() -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        git(dir.path(), &["init", "-q"]);
        std::fs::write(dir.path().join("README.md"), "hello").expect("write");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-qm", "init"]);
        dir
    }

    #[test]
    fn the_clone_is_created_once_and_reused() {
        let host = repository();
        let data = TempDir::new().expect("data");
        let root = data.path().join("project");

        super::ensure(&root, host.path()).expect("clone");
        assert!(root.join(".jj").is_dir(), "the clone is a jj repo");
        assert!(root.join(".git").exists(), "and it is co-located");

        super::ensure(&root, host.path()).expect("a second create reuses it");
    }

    #[test]
    fn the_paths_are_keyed_by_slug_and_sandbox() {
        let root = super::root("project");
        assert!(root.ends_with("vcs/jj/project"));
        assert!(super::workspace(&root, "demo").ends_with("workspaces/demo"));
        assert!(super::reconcile(&root, "demo").ends_with("reconcile/demo"));
    }
}
