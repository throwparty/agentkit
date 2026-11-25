use std::collections::HashSet;

use git2::{Oid, Repository};

use crate::domain::SandboxError;
use crate::vcs::HistoryStep;
use crate::vcs::git_common::GitError;

use super::refs;

/// Read the commits the shared branch pointed at.
///
/// The branch reflog is newest first. A commit that appears twice is listed
/// once. A commit whose object is gone is skipped. The synced reflog only
/// labels a step; it never adds one.
pub(super) fn read(repo: &Repository, branch: &str) -> Result<Vec<HistoryStep>, SandboxError> {
    let tip = repo
        .refname_to_id(&refs::branch(branch))
        .map_err(|source| {
            if source.code() == git2::ErrorCode::NotFound {
                SandboxError::SandboxNotFound {
                    name: branch.to_string(),
                }
            } else {
                SandboxError::from(GitError::Reference { source })
            }
        })?;
    let tip_commit = repo
        .find_commit(tip)
        .map_err(|source| SandboxError::from(GitError::Commit { source }))?;
    let synced = synced_ids(repo, branch);

    let mut steps = Vec::new();
    let mut seen = HashSet::new();

    if let Ok(reflog) = repo.reflog(&refs::branch(branch)) {
        for entry in reflog.iter() {
            let id = entry.id_new();
            if id == Oid::ZERO_SHA1 || !seen.insert(id) {
                continue;
            }
            if repo.find_commit(id).is_err() {
                continue;
            }
            steps.push(HistoryStep {
                revision: crate::vcs::Revision::new(id.to_string()),
                time: entry.committer().when().seconds(),
                description: entry
                    .message()
                    .ok()
                    .flatten()
                    .unwrap_or("the branch moved")
                    .to_string(),
                synced: synced.contains(&id),
            });
        }
    }

    let tip_text = tip.to_string();
    if steps.first().map(|step| step.revision.as_str()) != Some(tip_text.as_str()) {
        steps.retain(|step| step.revision.as_str() != tip_text.as_str());
        steps.insert(
            0,
            HistoryStep {
                revision: crate::vcs::Revision::new(tip_text),
                time: tip_commit.time().seconds(),
                description: "the shared branch tip".to_string(),
                synced: synced.contains(&tip),
            },
        );
    }

    Ok(steps)
}

/// The commits the synced ref has pointed at.
fn synced_ids(repo: &Repository, branch: &str) -> HashSet<Oid> {
    let mut ids = HashSet::new();
    if let Ok(reflog) = repo.reflog(&refs::synced(branch)) {
        for entry in reflog.iter() {
            let id = entry.id_new();
            if id != Oid::ZERO_SHA1 {
                ids.insert(id);
            }
        }
    }
    ids
}
