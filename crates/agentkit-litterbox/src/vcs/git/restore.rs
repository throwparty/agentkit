use git2::{Oid, Repository, Signature};

use crate::domain::SandboxError;
use crate::vcs::git_common::GitError;
use crate::vcs::{Restored, Revision};

use super::refs;

/// Refuse to move a shared branch while any worktree has it checked out.
///
/// The checkout's index and files would otherwise disagree with its HEAD.
pub(super) fn preflight(repo: &Repository, branch: &str) -> Result<(), SandboxError> {
    let reference = refs::branch(branch);
    let mut checked_out: Vec<std::path::PathBuf> = Vec::new();

    if !repo.is_bare()
        && repo
            .head()
            .ok()
            .and_then(|head| head.shorthand().ok().map(|name| name == branch))
            .unwrap_or(false)
        && let Some(workdir) = repo.workdir()
    {
        checked_out.push(workdir.to_path_buf());
    }

    if let Ok(names) = repo.worktrees() {
        for name in names.iter() {
            let Ok(Some(name)) = name else {
                continue;
            };
            let admin = repo.path().join("worktrees").join(name);
            let Ok(head) = std::fs::read_to_string(admin.join("HEAD")) else {
                continue;
            };
            if head.trim() == format!("ref: {reference}") {
                let path = std::fs::read_to_string(admin.join("gitdir"))
                    .ok()
                    .map(|gitdir| std::path::PathBuf::from(gitdir.trim()))
                    .and_then(|gitdir| gitdir.parent().map(std::path::Path::to_path_buf))
                    .unwrap_or_else(|| admin.clone());
                checked_out.push(path);
            }
        }
    }

    if let Some(path) = checked_out.first() {
        return Err(SandboxError::Vcs {
            message: format!(
                "the shared branch {branch} is checked out in {}; switch that worktree away \
                 before restoring",
                path.display()
            ),
        });
    }
    Ok(())
}

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
        // The tree already matches, but the shared ref must still be the tip
        // this comparison was made against. A push in between must not be
        // reported as a successful restore of the old revision.
        refs::write_matching(
            repo,
            &branch_ref,
            tip,
            tip,
            "litterbox: verify the shared branch before aligning the sandbox",
        )?;
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
