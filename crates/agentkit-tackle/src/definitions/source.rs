//! Source resolution: git checkouts through `git2` and local directories.
//!
//! Git is already a dependency for project identity, so sourcing adds no
//! new runtime dependency. A checkout is immutable for the life of the
//! operation: the requested ref is resolved to a commit and the working
//! tree detached at that commit.

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("{label}: failed to fetch {url}: {message}")]
    Fetch {
        label: String,
        url: String,
        message: String,
    },
    #[error("{label}: unknown ref `{reference}` in {url}")]
    UnknownRef {
        label: String,
        url: String,
        reference: String,
    },
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// A git source checked out at a resolved commit. Owning the checkout
/// directory removes it on drop.
#[derive(Debug)]
pub struct GitCheckout {
    path: PathBuf,
    commit: String,
}

impl Drop for GitCheckout {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A unique scratch directory for a checkout, under the platform temp dir.
fn scratch_dir() -> Result<PathBuf, SourceError> {
    let path = std::env::temp_dir().join(format!("tackle-definitions-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).map_err(|source| SourceError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

impl GitCheckout {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The immutable commit the ref resolved to.
    pub fn commit(&self) -> &str {
        &self.commit
    }
}

/// Clones `url` and detaches the working tree at `reference` (or `HEAD`).
/// An unresolvable ref fails with the source name and URL.
pub fn resolve_git(
    source: &str,
    url: &str,
    reference: Option<&str>,
) -> Result<GitCheckout, SourceError> {
    let path = scratch_dir()?;
    let repo = git2::Repository::clone(url, &path).map_err(|err| SourceError::Fetch {
        label: source.to_owned(),
        url: url.to_owned(),
        message: err.message().to_owned(),
    })?;

    let object = match reference {
        Some(reference) => repo
            .revparse_single(reference)
            .or_else(|_| repo.revparse_single(&format!("refs/remotes/origin/{reference}")))
            .or_else(|_| repo.revparse_single(&format!("refs/tags/{reference}")))
            .map_err(|_| SourceError::UnknownRef {
                label: source.to_owned(),
                url: url.to_owned(),
                reference: reference.to_owned(),
            })?,
        None => repo
            .revparse_single("HEAD")
            .map_err(|err| SourceError::Fetch {
                label: source.to_owned(),
                url: url.to_owned(),
                message: err.message().to_owned(),
            })?,
    };
    let commit = object.peel_to_commit().map_err(|err| SourceError::Fetch {
        label: source.to_owned(),
        url: url.to_owned(),
        message: err.message().to_owned(),
    })?;

    repo.set_head_detached(commit.id())
        .and_then(|()| repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())))
        .map_err(|err| SourceError::Fetch {
            label: source.to_owned(),
            url: url.to_owned(),
            message: err.message().to_owned(),
        })?;

    Ok(GitCheckout {
        path,
        commit: commit.id().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_with_tag() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        std::fs::write(dir.path().join("definitions.toml"), "# fixture\n").unwrap();
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = git2::Signature::now("test", "test@example.com").unwrap();
        let first = repo
            .commit(Some("HEAD"), &signature, &signature, "first", &tree, &[])
            .unwrap();
        repo.tag_lightweight("v1", &repo.find_object(first, None).unwrap(), false)
            .unwrap();
        let first = first.to_string();
        (dir, first)
    }

    #[test]
    fn a_tag_resolves_to_its_commit() {
        let (dir, commit) = repo_with_tag();
        let checkout = resolve_git("team", dir.path().to_str().unwrap(), Some("v1")).unwrap();
        assert_eq!(checkout.commit(), commit);
        assert!(checkout.path().join("definitions.toml").is_file());
    }

    #[test]
    fn a_commit_resolves_directly() {
        let (dir, commit) = repo_with_tag();
        let checkout = resolve_git("team", dir.path().to_str().unwrap(), Some(&commit)).unwrap();
        assert_eq!(checkout.commit(), commit);
    }

    #[test]
    fn a_branch_resolves_to_its_tip() {
        let (dir, first) = repo_with_tag();
        let repo = git2::Repository::open(dir.path()).unwrap();
        let first = repo
            .find_commit(git2::Oid::from_str(&first).unwrap())
            .unwrap();
        repo.branch("feature", &first, false).unwrap();
        repo.set_head("refs/heads/feature").unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        std::fs::write(dir.path().join("second.md"), "second\n").unwrap();
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = git2::Signature::now("test", "test@example.com").unwrap();
        let second = repo
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                "second",
                &tree,
                &[&first],
            )
            .unwrap();

        let checkout = resolve_git("team", dir.path().to_str().unwrap(), Some("feature")).unwrap();
        assert_eq!(checkout.commit(), second.to_string());
        assert!(checkout.path().join("second.md").is_file());
    }

    #[test]
    fn an_unknown_ref_errors_naming_the_source() {
        let (dir, _) = repo_with_tag();
        let err = resolve_git("team", dir.path().to_str().unwrap(), Some("missing")).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("team"), "{message}");
        assert!(message.contains("missing"), "{message}");
        assert!(message.contains(dir.path().to_str().unwrap()), "{message}");
    }

    #[test]
    fn an_unreachable_source_errors_naming_the_source_and_url() {
        let err = resolve_git("team", "/nonexistent/repository", None).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("team"), "{message}");
        assert!(message.contains("/nonexistent/repository"), "{message}");
    }
}
