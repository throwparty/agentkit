//! The three refs that pair a sandbox with a branch.
//!
//! The shared branch is the collaboration point. The sandbox branch is the
//! agent's own line and the `ours` side of a host merge. The synced ref is the
//! revision both sides last agreed on and the base of that merge.

use git2::{Oid, Repository};

use crate::domain::SandboxError;
use crate::vcs::git_common::GitError;

/// The ref that holds the shared branch.
pub fn branch(branch: &str) -> String {
    format!("refs/heads/{branch}")
}

/// The ref that holds the sandbox branch.
pub fn sandbox(branch: &str) -> String {
    format!("refs/litterbox/workspaces/{}", slug(branch))
}

/// The ref that holds the synced revision.
pub fn synced(branch: &str) -> String {
    format!("refs/litterbox/synced/{}", slug(branch))
}

fn slug(branch: &str) -> &str {
    branch.strip_prefix("litterbox/").unwrap_or(branch)
}

/// The commit a ref points at, when it exists.
pub(crate) fn read(repo: &Repository, reference: &str) -> Result<Option<Oid>, SandboxError> {
    match repo.refname_to_id(reference) {
        Ok(oid) => Ok(Some(oid)),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(source) => Err(SandboxError::from(GitError::Reference { source })),
    }
}

/// Point a ref at a commit, writing a reflog entry.
pub(crate) fn write(
    repo: &Repository,
    reference: &str,
    oid: Oid,
    message: &str,
) -> Result<(), SandboxError> {
    repo.reference(reference, oid, true, message)
        .map(|_| ())
        .map_err(|source| SandboxError::from(GitError::Reference { source }))
}

/// Point a ref at a commit only if it still points at `expected`.
pub(crate) fn write_matching(
    repo: &Repository,
    reference: &str,
    oid: Oid,
    expected: Oid,
    message: &str,
) -> Result<(), SandboxError> {
    match repo.reference_matching(reference, oid, true, expected, message) {
        Ok(_) => Ok(()),
        Err(error)
            if matches!(
                error.code(),
                git2::ErrorCode::Modified | git2::ErrorCode::Exists | git2::ErrorCode::NotFound
            ) =>
        {
            let current = repo
                .refname_to_id(reference)
                .map(|oid| oid.to_string())
                .unwrap_or_else(|_| "none".to_string());
            Err(SandboxError::BranchMoved {
                expected: expected.to_string(),
                current,
            })
        }
        Err(source) => Err(SandboxError::from(GitError::Reference { source })),
    }
}

/// Remove a ref, ignoring one that is already gone.
pub(crate) fn delete(repo: &Repository, reference: &str) -> Result<(), SandboxError> {
    match repo.find_reference(reference) {
        Ok(mut found) => found
            .delete()
            .map_err(|source| SandboxError::from(GitError::Reference { source })),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(()),
        Err(source) => Err(SandboxError::from(GitError::Reference { source })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refs_keep_the_litterbox_prefix() {
        assert_eq!(branch("litterbox/demo"), "refs/heads/litterbox/demo");
        assert_eq!(sandbox("litterbox/demo"), "refs/litterbox/workspaces/demo");
        assert_eq!(synced("litterbox/demo"), "refs/litterbox/synced/demo");
    }

    #[test]
    fn a_branch_without_the_prefix_is_still_addressed() {
        assert_eq!(sandbox("demo"), "refs/litterbox/workspaces/demo");
        assert_eq!(synced("demo"), "refs/litterbox/synced/demo");
    }
}
