use std::path::Path;

use crate::domain::SandboxError;

const REMOTE_NAME: &str = "litterbox";

pub fn install_remote(repo_path: &Path, bare_path: &Path) -> Result<(), SandboxError> {
    let repo = git2::Repository::open(repo_path)
        .map_err(|source| SandboxError::Scm(crate::domain::ScmError::Open { source }))?;

    let bare_url = bare_path_to_url(bare_path);

    match repo.find_remote(REMOTE_NAME) {
        Ok(remote) => {
            let existing = remote.url().unwrap_or("");
            if existing == bare_url {
                Ok(())
            } else {
                Err(SandboxError::Config(format!(
                    "litterbox remote already exists with URL '{existing}', cannot override with '{bare_url}'"
                )))
            }
        }
        Err(e) if e.code() == git2::ErrorCode::NotFound => {
            repo.remote(REMOTE_NAME, &bare_url)
                .map_err(|source| SandboxError::Scm(crate::domain::ScmError::BranchCreate { source }))?;
            Ok(())
        }
        Err(source) => Err(SandboxError::Scm(crate::domain::ScmError::Open { source })),
    }
}

pub fn remove_remote(repo_path: &Path) -> Result<(), SandboxError> {
    let repo = git2::Repository::open(repo_path)
        .map_err(|source| SandboxError::Scm(crate::domain::ScmError::Open { source }))?;

    match repo.find_remote(REMOTE_NAME) {
        Ok(_) => repo
            .remote_delete(REMOTE_NAME)
            .map_err(|source| SandboxError::Scm(crate::domain::ScmError::BranchDelete { source })),
        Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(()),
        Err(source) => Err(SandboxError::Scm(crate::domain::ScmError::Open { source })),
    }
}

fn bare_path_to_url(bare_path: &Path) -> String {
    let canonical = if bare_path.is_absolute() {
        bare_path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_default()
            .join(bare_path)
    };
    canonical.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use tempfile::TempDir;

    use git2::Repository;

    fn init_repo() -> (TempDir, Repository) {
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
    fn install_remote_adds_litterbox_remote() {
        let (tempdir, _repo) = init_repo();
        let bare_path = TempDir::new().expect("bare dir");
        let result = install_remote(tempdir.path(), bare_path.path());
        assert!(result.is_ok());

        let repo = Repository::open(tempdir.path()).expect("open repo");
        let remote = repo.find_remote("litterbox").expect("remote exists");
        let url = remote.url().expect("url");
        assert!(url.contains(bare_path.path().to_str().unwrap()));
    }

    #[test]
    fn install_remote_is_idempotent() {
        let (tempdir, _repo) = init_repo();
        let bare_path = TempDir::new().expect("bare dir");

        install_remote(tempdir.path(), bare_path.path()).expect("first install");
        let second = install_remote(tempdir.path(), bare_path.path());
        assert!(second.is_ok());

        // Still only one remote
        let repo = Repository::open(tempdir.path()).expect("open repo");
        let remote = repo.find_remote("litterbox").expect("remote exists");
        assert_eq!(remote.url().unwrap_or(""), bare_path.path().to_str().unwrap());
    }

    #[test]
    fn install_remote_errors_on_conflicting_url() {
        let (tempdir, _repo) = init_repo();
        let first_bare = TempDir::new().expect("first bare");
        let second_bare = TempDir::new().expect("second bare");

        install_remote(tempdir.path(), first_bare.path()).expect("first install");
        let err = install_remote(tempdir.path(), second_bare.path())
            .expect_err("conflicting url");

        assert!(err.to_string().contains("already exists"));
        assert!(err.to_string().contains("cannot override"));
    }

    #[test]
    fn remove_remote_removes_litterbox() {
        let (tempdir, _repo) = init_repo();
        let bare_path = TempDir::new().expect("bare dir");

        install_remote(tempdir.path(), bare_path.path()).expect("install");
        remove_remote(tempdir.path()).expect("remove");

        let repo = Repository::open(tempdir.path()).expect("open repo");
        assert!(repo.find_remote("litterbox").is_err());
    }

    #[test]
    fn remove_remote_noop_when_missing() {
        let (tempdir, _repo) = init_repo();
        let result = remove_remote(tempdir.path());
        assert!(result.is_ok());
    }
}
