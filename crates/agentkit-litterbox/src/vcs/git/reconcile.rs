//! The host commands: report the divergence and stage the merge.
//!
//! These run the `git` binary because libgit2 cannot merge with an explicit
//! base or write a conflicted working tree with markers. Every command runs
//! with an empty hooks path.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::vcs::{Error as VcsError, Finished, Report, Revision, Staged};

/// Where a sandbox's staging worktree lives.
pub(crate) fn worktree_path(root: &Path, project: &str, sandbox: &str) -> PathBuf {
    root.join(project).join(sandbox)
}

/// Run `git` in `repo` with hooks disabled.
fn git(repo: &Path, args: &[&str]) -> Result<Output, VcsError> {
    let hooks = tempfile::tempdir().map_err(VcsError::Io)?;
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("-c")
        .arg(format!("core.hooksPath={}", hooks.path().display()))
        .args(args)
        .output()
        .map_err(|error| VcsError::Backend {
            operation: "run git".to_string(),
            message: format!("could not run git: {error}"),
        })
}

fn failed(operation: &str, output: &Output) -> VcsError {
    VcsError::Backend {
        operation: operation.to_string(),
        message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    }
}

fn text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Create the conflicted merge revision for a sandbox and report it.
///
/// The first parent is the sandbox branch, so the sides read `ours` from the
/// sandbox and `theirs` from the shared branch. The tree comes from
/// `merge-tree`, which writes markers without needing a worktree.
pub(crate) fn begin(
    repo: &Path,
    base: &Revision,
    ours: &Revision,
    theirs: &Revision,
) -> Result<Revision, VcsError> {
    let merged = git(
        repo,
        &[
            "merge-tree",
            "--write-tree",
            &format!("--merge-base={}", base.as_str()),
            ours.as_str(),
            theirs.as_str(),
        ],
    )?;
    let stdout = String::from_utf8_lossy(&merged.stdout);
    let tree = stdout.lines().next().unwrap_or_default().trim().to_string();
    if tree.is_empty() {
        return Err(failed("create the conflict tree", &merged));
    }
    let commit = git(
        repo,
        &[
            "commit-tree",
            &tree,
            "-p",
            ours.as_str(),
            "-p",
            theirs.as_str(),
            "-m",
            "litterbox: resolve the divergence",
        ],
    )?;
    if !commit.status.success() {
        return Err(failed("create the conflict revision", &commit));
    }
    Ok(Revision::new(text(&commit)))
}

/// The conflicted paths in a committed tree, read from its markers.
///
/// Git has no conflicted commit, so a marker tree is the only evidence that a
/// conflict reached history. A region opens at `<<<<<<<`, and `|||||||` marks a
/// common base. The scan is the safety net behind the host staging path.
pub(crate) fn marker_conflicts(
    repo: &Path,
    revision: &str,
) -> Result<Vec<crate::vcs::Conflict>, VcsError> {
    let output = git(
        repo,
        &[
            "grep",
            "--no-color",
            "-n",
            "-I",
            "-E",
            r"^(<<<<<<<|\|\|\|\|\|\|\|)",
            revision,
            "--",
            ".",
        ],
    )?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(failed("scan the revision for conflict markers", &output));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut found: Vec<crate::vcs::Conflict> = Vec::new();
    for line in stdout.lines() {
        let mut parts = line.splitn(4, ':');
        let (_, path, _, content) = (parts.next(), parts.next(), parts.next(), parts.next());
        let (Some(path), Some(content)) = (path, content) else {
            continue;
        };
        let entry = found
            .iter_mut()
            .find(|conflict| conflict.path == Path::new(path));
        if content.starts_with("<<<<<<<") {
            match entry {
                Some(conflict) => conflict.hunks += 1,
                None => found.push(crate::vcs::Conflict {
                    path: PathBuf::from(path),
                    hunks: 1,
                    sides: 2,
                    base: false,
                }),
            }
        } else if content.starts_with("|||||||")
            && let Some(conflict) = entry
        {
            conflict.base = true;
        }
    }
    Ok(found)
}

/// The conflict paths in a `merge-tree` result.
fn conflict_paths(stdout: &str) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for line in stdout.lines() {
        let Some((meta, path)) = line.split_once('\t') else {
            continue;
        };
        let stage = meta.split_whitespace().last();
        if matches!(stage, Some("1" | "2" | "3"))
            && !paths.iter().any(|seen| seen == Path::new(path))
        {
            paths.push(PathBuf::from(path));
        }
    }
    paths
}

/// Report the divergence without writing a ref or a checkout file.
pub(crate) fn report(
    repo: &Path,
    synced: &Revision,
    branch: &Revision,
    sandbox: &Revision,
    reflog: String,
    worktree: Option<PathBuf>,
) -> Result<Report, VcsError> {
    let base = format!("--merge-base={}", synced.as_str());
    let output = git(
        repo,
        &[
            "merge-tree",
            "--write-tree",
            &base,
            branch.as_str(),
            sandbox.as_str(),
        ],
    )?;

    let conflicts = match output.status.code() {
        Some(0) => Vec::new(),
        Some(1) => conflict_paths(&String::from_utf8_lossy(&output.stdout)),
        _ => return Err(failed("report the divergence", &output)),
    };

    Ok(Report {
        base: synced.clone(),
        branch: branch.clone(),
        sandbox: sandbox.clone(),
        conflicts,
        worktree,
        reflog: Some(reflog),
    })
}

/// Create the staging worktree and stop with the conflict in place.
pub(crate) fn stage(
    repo: &Path,
    path: &Path,
    synced: &Revision,
    branch: &Revision,
    sandbox: &Revision,
    temp_branch: &str,
) -> Result<Staged, VcsError> {
    if path.exists() {
        return Err(VcsError::Backend {
            operation: "stage the merge".to_string(),
            message: format!(
                "a staging worktree already exists at {}; finish or discard it first",
                path.display()
            ),
        });
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(VcsError::Io)?;
    }
    let path_text = path.to_str().ok_or_else(|| VcsError::Backend {
        operation: "stage the merge".to_string(),
        message: "the staging path is not UTF-8".to_string(),
    })?;

    let added = git(
        repo,
        &[
            "worktree",
            "add",
            "-b",
            temp_branch,
            path_text,
            branch.as_str(),
        ],
    )?;
    if !added.status.success() {
        return Err(failed("create the staging worktree", &added));
    }

    let result = stage_in(path, synced, branch, sandbox);
    if result.is_err() {
        let _ = git(repo, &["worktree", "remove", "--force", path_text]);
        let _ = git(repo, &["branch", "-D", temp_branch]);
    }
    result
}

fn stage_in(
    path: &Path,
    synced: &Revision,
    branch: &Revision,
    sandbox: &Revision,
) -> Result<Staged, VcsError> {
    let merged = git(
        path,
        &[
            "read-tree",
            "-m",
            synced.as_str(),
            branch.as_str(),
            sandbox.as_str(),
        ],
    )?;
    if !merged.status.success() {
        return Err(failed("merge the sandbox branch", &merged));
    }

    let checked_out = git(path, &["checkout", "--conflict=merge", "--", "."])?;
    if !checked_out.status.success() {
        return Err(failed("write the conflicted working tree", &checked_out));
    }

    let git_dir = git(path, &["rev-parse", "--git-dir"])?;
    if !git_dir.status.success() {
        return Err(failed("find the worktree git dir", &git_dir));
    }
    let git_dir = PathBuf::from(text(&git_dir));
    let git_dir = if git_dir.is_absolute() {
        git_dir
    } else {
        path.join(git_dir)
    };
    std::fs::write(
        git_dir.join("MERGE_HEAD"),
        format!("{}\n", sandbox.as_str()),
    )
    .map_err(VcsError::Io)?;
    std::fs::write(
        git_dir.join("MERGE_MSG"),
        "litterbox: merge the sandbox branch\n",
    )
    .map_err(VcsError::Io)?;

    let unmerged = git(path, &["diff", "--name-only", "--diff-filter=U"])?;
    if !unmerged.status.success() {
        return Err(failed("list the conflicts", &unmerged));
    }
    let conflicts = String::from_utf8_lossy(&unmerged.stdout)
        .lines()
        .map(PathBuf::from)
        .collect();

    Ok(Staged {
        path: path.to_path_buf(),
        branch: branch.clone(),
        conflicts,
    })
}

/// Whether any worktree holds `refs/heads/<branch>`.
fn worktree_holds(repo: &Path, branch: &str) -> Result<bool, VcsError> {
    let output = git(repo, &["worktree", "list", "--porcelain"])?;
    if !output.status.success() {
        return Err(failed("list worktrees", &output));
    }
    let wanted = format!("branch refs/heads/{branch}");
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.trim() == wanted))
}

/// Finish a staged merge.
pub(crate) fn finish(
    repo: &Path,
    path: &Path,
    branch: &str,
    branch_tip: &Revision,
    temp_branch: &str,
) -> Result<Finished, VcsError> {
    if !path.is_dir() {
        return Err(VcsError::Backend {
            operation: "finish the merge".to_string(),
            message: format!("no staging worktree at {}", path.display()),
        });
    }
    let path_text = path.to_str().ok_or_else(|| VcsError::Backend {
        operation: "finish the merge".to_string(),
        message: "the staging path is not UTF-8".to_string(),
    })?;

    let status = git(path, &["status", "--porcelain"])?;
    if !status.status.success() {
        return Err(failed("read the staging worktree", &status));
    }
    let unmerged = git(path, &["diff", "--name-only", "--diff-filter=U"])?;
    if !unmerged.status.success() {
        return Err(failed("read the staging worktree", &unmerged));
    }
    if !status.stdout.is_empty() || !unmerged.stdout.is_empty() {
        return Err(VcsError::Backend {
            operation: "finish the merge".to_string(),
            message: "the staging worktree has uncommitted or unresolved paths".to_string(),
        });
    }

    let head = git(path, &["rev-parse", "HEAD"])?;
    if !head.status.success() {
        return Err(failed("read the merge commit", &head));
    }
    let head = text(&head);

    if head != branch_tip.as_str() {
        let contains = git(
            repo,
            &["merge-base", "--is-ancestor", branch_tip.as_str(), &head],
        )?;
        if !contains.status.success() {
            return Err(VcsError::Backend {
                operation: "finish the merge".to_string(),
                message: format!("the merge commit {head} does not descend from {branch_tip}"),
            });
        }
    }

    if head != branch_tip.as_str() && worktree_holds(repo, branch)? {
        return Ok(Finished::Deferred {
            command: format!("git merge --ff-only {head}"),
        });
    }

    if head != branch_tip.as_str() {
        let moved = git(
            repo,
            &[
                "update-ref",
                &format!("refs/heads/{branch}"),
                &head,
                branch_tip.as_str(),
            ],
        )?;
        if !moved.status.success() {
            return Err(failed("move the branch", &moved));
        }
    }

    let _ = git(repo, &["worktree", "remove", path_text]);
    let _ = git(repo, &["branch", "-D", temp_branch]);

    Ok(Finished::Landed {
        branch: Revision::new(head),
    })
}

/// Remove a staging worktree and its temporary branch.
pub(crate) fn discard(repo: &Path, path: &Path, temp_branch: &str) -> Result<(), VcsError> {
    if let Some(path) = path.to_str()
        && path_is_dir(path)
    {
        let _ = git(repo, &["worktree", "remove", "--force", path]);
    }
    let _ = git(repo, &["branch", "-D", temp_branch]);
    let _ = git(repo, &["worktree", "prune"]);
    Ok(())
}

fn path_is_dir(path: &str) -> bool {
    Path::new(path).is_dir()
}
