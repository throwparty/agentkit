use std::path::{Path, PathBuf};

use git2::build::CheckoutBuilder;
use git2::{
    ApplyLocation, Diff, DiffFormat, DiffOptions, IndexAddOption, Oid, Repository, ResetType,
    Signature, Sort, Status, StatusOptions,
};

use crate::git_sdk::{
    AuthorInfo, CommitInfo, CommitRange, GitSdk, GitSdkError, GitSdkResult, StatusEntry, StatusKind,
};

#[derive(Debug, Default, Clone, Copy)]
pub struct Git2Sdk;

impl Git2Sdk {
    fn open_repo(&self, repo_path: &Path) -> GitSdkResult<Repository> {
        Repository::open(repo_path).map_err(GitSdkError::from)
    }

    fn signature(author: &AuthorInfo) -> GitSdkResult<Signature<'_>> {
        Signature::now(&author.name, &author.email).map_err(GitSdkError::from)
    }

    fn to_relative(repo_path: &Path, path: &str) -> GitSdkResult<PathBuf> {
        let candidate = Path::new(path);
        if candidate.is_absolute() {
            candidate
                .strip_prefix(repo_path)
                .map(|relative| relative.to_path_buf())
                .map_err(|_| GitSdkError::InvalidInput(format!("path not in repo: {path}")))
        } else {
            Ok(candidate.to_path_buf())
        }
    }

    fn status_kind(status: Status) -> Option<StatusKind> {
        if status.is_conflicted() {
            return Some(StatusKind::Conflicted);
        }
        if status.is_wt_new() {
            return Some(StatusKind::Untracked);
        }
        if status.is_index_new() {
            return Some(StatusKind::Added);
        }
        if status.is_index_modified() || status.is_wt_modified() {
            return Some(StatusKind::Modified);
        }
        if status.is_index_deleted() || status.is_wt_deleted() {
            return Some(StatusKind::Deleted);
        }
        if status.is_index_renamed() || status.is_wt_renamed() {
            return Some(StatusKind::Renamed);
        }
        if status.is_index_typechange() || status.is_wt_typechange() {
            return Some(StatusKind::Modified);
        }
        None
    }
}

impl From<git2::Error> for GitSdkError {
    fn from(value: git2::Error) -> Self {
        Self::Git(value.message().to_string())
    }
}

impl GitSdk for Git2Sdk {
    fn init(&self, path: &Path) -> GitSdkResult<()> {
        Repository::init(path)
            .map(|_| ())
            .map_err(GitSdkError::from)
    }

    fn add(&self, repo_path: &Path, paths: &[String]) -> GitSdkResult<()> {
        let repo = self.open_repo(repo_path)?;
        let mut index = repo.index().map_err(GitSdkError::from)?;
        for path in paths {
            let relative = Self::to_relative(repo_path, path)?;
            index.add_path(&relative).map_err(GitSdkError::from)?;
        }
        index.write().map_err(GitSdkError::from)?;
        Ok(())
    }

    fn status(&self, repo_path: &Path) -> GitSdkResult<Vec<StatusEntry>> {
        let repo = self.open_repo(repo_path)?;
        let mut options = StatusOptions::new();
        options.include_untracked(true).recurse_untracked_dirs(true);
        let statuses = repo
            .statuses(Some(&mut options))
            .map_err(GitSdkError::from)?;
        let mut entries = Vec::new();
        for entry in statuses.iter() {
            let Some(path) = entry.path() else {
                continue;
            };
            if let Some(kind) = Self::status_kind(entry.status()) {
                entries.push(StatusEntry {
                    path: path.to_string(),
                    status: kind,
                });
            }
        }
        Ok(entries)
    }

    fn commit(&self, repo_path: &Path, message: &str, author: &AuthorInfo) -> GitSdkResult<String> {
        let repo = self.open_repo(repo_path)?;
        let mut index = repo.index().map_err(GitSdkError::from)?;
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .map_err(GitSdkError::from)?;
        index.write().map_err(GitSdkError::from)?;
        let tree_id = index.write_tree().map_err(GitSdkError::from)?;
        let tree = repo.find_tree(tree_id).map_err(GitSdkError::from)?;
        let signature = Self::signature(author)?;

        let parent = repo.head().ok().and_then(|head| head.target());
        let commit_id = if let Some(parent_id) = parent {
            let parent_commit = repo.find_commit(parent_id).map_err(GitSdkError::from)?;
            repo.commit(
                Some("HEAD"),
                &signature,
                &signature,
                message,
                &tree,
                &[&parent_commit],
            )
            .map_err(GitSdkError::from)?
        } else {
            repo.commit(Some("HEAD"), &signature, &signature, message, &tree, &[])
                .map_err(GitSdkError::from)?
        };

        Ok(commit_id.to_string())
    }

    fn log(&self, repo_path: &Path, max: usize) -> GitSdkResult<Vec<CommitInfo>> {
        let repo = self.open_repo(repo_path)?;
        let mut revwalk = repo.revwalk().map_err(GitSdkError::from)?;
        if revwalk.push_head().is_err() {
            return Ok(Vec::new());
        }
        revwalk.set_sorting(Sort::TIME).map_err(GitSdkError::from)?;

        let mut commits = Vec::new();
        for oid in revwalk.take(max) {
            let oid = oid.map_err(GitSdkError::from)?;
            let commit = repo.find_commit(oid).map_err(GitSdkError::from)?;
            let author = commit.author();
            commits.push(CommitInfo {
                id: commit.id().to_string(),
                message: commit.message().unwrap_or("").trim_end().to_string(),
                author: AuthorInfo {
                    name: author.name().unwrap_or("").to_string(),
                    email: author.email().unwrap_or("").to_string(),
                },
                timestamp: commit.time().seconds(),
            });
        }
        Ok(commits)
    }

    fn branch(&self, repo_path: &Path, name: &str, target: Option<&str>) -> GitSdkResult<()> {
        let repo = self.open_repo(repo_path)?;
        let target_commit = if let Some(reference) = target {
            let obj = repo.revparse_single(reference).map_err(GitSdkError::from)?;
            obj.peel_to_commit().map_err(GitSdkError::from)?
        } else {
            let head = repo.head().map_err(GitSdkError::from)?;
            let oid = head
                .target()
                .ok_or_else(|| GitSdkError::Git("HEAD missing".to_string()))?;
            repo.find_commit(oid).map_err(GitSdkError::from)?
        };
        repo.branch(name, &target_commit, false)
            .map(|_| ())
            .map_err(GitSdkError::from)
    }

    fn checkout(&self, repo_path: &Path, reference: &str) -> GitSdkResult<()> {
        let repo = self.open_repo(repo_path)?;

        let reference_name = if repo.find_reference(reference).is_ok() {
            reference.to_string()
        } else {
            format!("refs/heads/{reference}")
        };

        let reference = repo
            .find_reference(&reference_name)
            .map_err(GitSdkError::from)?;
        let target = reference.peel_to_tree().map_err(GitSdkError::from)?;
        let mut checkout = CheckoutBuilder::new();
        checkout.force();
        repo.checkout_tree(target.as_object(), Some(&mut checkout))
            .map_err(GitSdkError::from)?;
        repo.set_head(&reference_name).map_err(GitSdkError::from)?;
        Ok(())
    }

    fn squash(
        &self,
        repo_path: &Path,
        range: &CommitRange,
        message: &str,
        author: &AuthorInfo,
    ) -> GitSdkResult<String> {
        let repo = self.open_repo(repo_path)?;
        let start = repo
            .find_commit(Oid::from_str(&range.start).map_err(GitSdkError::from)?)
            .map_err(GitSdkError::from)?;
        let end = repo
            .find_commit(Oid::from_str(&range.end).map_err(GitSdkError::from)?)
            .map_err(GitSdkError::from)?;

        let signature = Self::signature(author)?;
        let end_tree = end.tree().map_err(GitSdkError::from)?;

        let parents: Vec<git2::Commit<'_>> = if start.parent_count() == 0 {
            Vec::new()
        } else {
            vec![start.parent(0).map_err(GitSdkError::from)?]
        };

        let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();

        let commit_id = repo
            .commit(
                None,
                &signature,
                &signature,
                message,
                &end_tree,
                &parent_refs,
            )
            .map_err(GitSdkError::from)?;

        let commit = repo.find_commit(commit_id).map_err(GitSdkError::from)?;
        repo.reset(commit.as_object(), ResetType::Hard, None)
            .map_err(GitSdkError::from)?;

        Ok(commit_id.to_string())
    }

    fn diff(&self, repo_path: &Path, from: Option<&str>, to: Option<&str>) -> GitSdkResult<String> {
        let repo = self.open_repo(repo_path)?;
        let mut options = DiffOptions::new();

        let from_tree = match from {
            Some(reference) => {
                let obj = repo.revparse_single(reference).map_err(GitSdkError::from)?;
                Some(obj.peel_to_tree().map_err(GitSdkError::from)?)
            }
            None => None,
        };
        let to_tree = match to {
            Some(reference) => {
                let obj = repo.revparse_single(reference).map_err(GitSdkError::from)?;
                Some(obj.peel_to_tree().map_err(GitSdkError::from)?)
            }
            None => None,
        };

        let diff = repo
            .diff_tree_to_tree(from_tree.as_ref(), to_tree.as_ref(), Some(&mut options))
            .map_err(GitSdkError::from)?;

        diff_to_string(&diff)
    }

    fn apply_patch(&self, repo_path: &Path, patch: &str) -> GitSdkResult<()> {
        let repo = self.open_repo(repo_path)?;
        let diff = Diff::from_buffer(patch.as_bytes()).map_err(GitSdkError::from)?;
        repo.apply(&diff, ApplyLocation::WorkDir, None)
            .map_err(GitSdkError::from)?;
        Ok(())
    }
}

fn diff_to_string(diff: &Diff<'_>) -> GitSdkResult<String> {
    let mut buffer = Vec::new();
    diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
        let origin = line.origin();
        if origin == '+' || origin == '-' || origin == ' ' {
            buffer.push(origin as u8);
        }
        buffer.extend_from_slice(line.content());
        true
    })
    .map_err(GitSdkError::from)?;
    Ok(String::from_utf8_lossy(&buffer).to_string())
}
