use std::path::Path;

use gix::Repository;
use gix::bstr::ByteSlice;

use crate::git_cli_sdk::GitCliSdk;
use crate::git_sdk::{
    AuthorInfo, CommitInfo, CommitRange, GitSdk, GitSdkError, GitSdkResult, StatusEntry,
};

#[derive(Debug, Default, Clone, Copy)]
pub struct GixSdk;

#[derive(Debug, Default, Clone, Copy)]
pub struct GixNativeSdk;

impl GixSdk {
    fn cli(&self) -> GitCliSdk {
        GitCliSdk
    }
}

impl GixNativeSdk {
    fn open_repo(&self, repo_path: &Path) -> GitSdkResult<Repository> {
        gix::open(repo_path).map_err(|err| GitSdkError::Git(err.to_string()))
    }
}

impl GitSdk for GixSdk {
    fn init(&self, _path: &Path) -> GitSdkResult<()> {
        self.cli().init(_path)
    }

    fn add(&self, _repo_path: &Path, _paths: &[String]) -> GitSdkResult<()> {
        self.cli().add(_repo_path, _paths)
    }

    fn status(&self, _repo_path: &Path) -> GitSdkResult<Vec<StatusEntry>> {
        self.cli().status(_repo_path)
    }

    fn commit(
        &self,
        _repo_path: &Path,
        _message: &str,
        _author: &AuthorInfo,
    ) -> GitSdkResult<String> {
        self.cli().commit(_repo_path, _message, _author)
    }

    fn log(&self, _repo_path: &Path, _max: usize) -> GitSdkResult<Vec<CommitInfo>> {
        self.cli().log(_repo_path, _max)
    }

    fn branch(&self, _repo_path: &Path, _name: &str, _target: Option<&str>) -> GitSdkResult<()> {
        self.cli().branch(_repo_path, _name, _target)
    }

    fn checkout(&self, _repo_path: &Path, _reference: &str) -> GitSdkResult<()> {
        self.cli().checkout(_repo_path, _reference)
    }

    fn squash(
        &self,
        _repo_path: &Path,
        _range: &CommitRange,
        _message: &str,
        _author: &AuthorInfo,
    ) -> GitSdkResult<String> {
        self.cli().squash(_repo_path, _range, _message, _author)
    }

    fn diff(
        &self,
        _repo_path: &Path,
        _from: Option<&str>,
        _to: Option<&str>,
    ) -> GitSdkResult<String> {
        self.cli().diff(_repo_path, _from, _to)
    }

    fn apply_patch(&self, _repo_path: &Path, _patch: &str) -> GitSdkResult<()> {
        self.cli().apply_patch(_repo_path, _patch)
    }
}

impl GitSdk for GixNativeSdk {
    fn init(&self, path: &Path) -> GitSdkResult<()> {
        gix::init(path)
            .map(|_| ())
            .map_err(|err| GitSdkError::Git(err.to_string()))
    }

    fn add(&self, _repo_path: &Path, _paths: &[String]) -> GitSdkResult<()> {
        Err(GitSdkError::Unsupported("gix add not implemented"))
    }

    fn status(&self, _repo_path: &Path) -> GitSdkResult<Vec<StatusEntry>> {
        Err(GitSdkError::Unsupported("gix status not implemented"))
    }

    fn commit(
        &self,
        _repo_path: &Path,
        _message: &str,
        _author: &AuthorInfo,
    ) -> GitSdkResult<String> {
        Err(GitSdkError::Unsupported("gix commit not implemented"))
    }

    fn log(&self, repo_path: &Path, max: usize) -> GitSdkResult<Vec<CommitInfo>> {
        let repo = self.open_repo(repo_path)?;
        let head_commit = match repo.head_commit() {
            Ok(commit) => commit,
            Err(_) => return Ok(Vec::new()),
        };

        let walk = repo
            .rev_walk([head_commit.id])
            .all()
            .map_err(|err| GitSdkError::Git(err.to_string()))?;

        let mut commits = Vec::new();
        for info in walk.take(max) {
            let info = info.map_err(|err| GitSdkError::Git(err.to_string()))?;
            let commit = info
                .object()
                .map_err(|err| GitSdkError::Git(err.to_string()))?;
            let commit_ref = gix::objs::CommitRef::from_bytes(&commit.data)
                .map_err(|err| GitSdkError::Git(err.to_string()))?;
            let author = commit_ref.author();
            commits.push(CommitInfo {
                id: info.id.to_string(),
                message: commit_ref.message().title.to_str_lossy().to_string(),
                author: AuthorInfo {
                    name: author.name.to_str_lossy().to_string(),
                    email: author.email.to_str_lossy().to_string(),
                },
                timestamp: author.time.seconds,
            });
        }
        Ok(commits)
    }

    fn branch(&self, _repo_path: &Path, _name: &str, _target: Option<&str>) -> GitSdkResult<()> {
        Err(GitSdkError::Unsupported("gix branch not implemented"))
    }

    fn checkout(&self, _repo_path: &Path, _reference: &str) -> GitSdkResult<()> {
        Err(GitSdkError::Unsupported("gix checkout not implemented"))
    }

    fn squash(
        &self,
        _repo_path: &Path,
        _range: &CommitRange,
        _message: &str,
        _author: &AuthorInfo,
    ) -> GitSdkResult<String> {
        Err(GitSdkError::Unsupported("gix squash not implemented"))
    }

    fn diff(
        &self,
        _repo_path: &Path,
        _from: Option<&str>,
        _to: Option<&str>,
    ) -> GitSdkResult<String> {
        Err(GitSdkError::Unsupported("gix diff not implemented"))
    }

    fn apply_patch(&self, _repo_path: &Path, _patch: &str) -> GitSdkResult<()> {
        Err(GitSdkError::Unsupported("gix apply_patch not implemented"))
    }
}
