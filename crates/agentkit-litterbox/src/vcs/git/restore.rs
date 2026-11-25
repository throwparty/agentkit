use git2::{Oid, Repository, Signature};

use crate::domain::SandboxError;
use crate::vcs::git_common::GitError;
use crate::vcs::{Restored, Revision};

use super::refs;

/// Add one commit on the shared branch whose tree is `target`, then move the
/// sandbox branch and `synced` to it.
pub(super) fn restore(
    repo: &Repository,
    branch: &str,
    target: &Revision,
) -> Result<Restored, SandboxError> {
    let branch_ref = refs::branch(branch);
    let tip = repo
        .refname_to_id(&branch_ref)
        .map_err(|source| SandboxError::from(GitError::Reference { source }))?;
    let target = target
        .as_str()
        .parse::<Oid>()
        .map_err(|source| SandboxError::from(GitError::Commit { source }))?;
    let target_commit = repo
        .find_commit(target)
        .map_err(|source| SandboxError::from(GitError::Commit { source }))?;
    let tip_commit = repo
        .find_commit(tip)
        .map_err(|source| SandboxError::from(GitError::Commit { source }))?;

    if target_commit.tree_id() == tip_commit.tree_id() {
        refs::write(
            repo,
            &refs::sandbox(branch),
            tip,
            "litterbox: align the sandbox branch",
        )?;
        refs::write(
            repo,
            &refs::synced(branch),
            tip,
            "litterbox: align the synced revision",
        )?;
        return Ok(Restored {
            revision: Revision::new(tip.to_string()),
            created: false,
        });
    }

    let signature = signature(repo)?;
    let tree = target_commit
        .tree()
        .map_err(|source| SandboxError::from(GitError::Commit { source }))?;
    let message = format!("litterbox: restore {branch} to {target}");
    let commit = repo
        .commit(
            None,
            &signature,
            &signature,
            &message,
            &tree,
            &[&tip_commit],
        )
        .map_err(|source| SandboxError::from(GitError::Commit { source }))?;

    refs::write_matching(
        repo,
        &branch_ref,
        commit,
        tip,
        "litterbox: restore the branch",
    )?;
    refs::write(
        repo,
        &refs::sandbox(branch),
        commit,
        "litterbox: restore the sandbox branch",
    )?;
    refs::write(
        repo,
        &refs::synced(branch),
        commit,
        "litterbox: record the restore",
    )?;

    Ok(Restored {
        revision: Revision::new(commit.to_string()),
        created: true,
    })
}

fn signature(repo: &Repository) -> Result<Signature<'_>, SandboxError> {
    repo.signature()
        .or_else(|_| Signature::now("Litterbox", "noreply@example.com"))
        .map_err(|source| SandboxError::from(GitError::Signature { source }))
}
