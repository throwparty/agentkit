use std::fmt;
use std::path::Path;

pub type GitSdkResult<T> = Result<T, GitSdkError>;

#[derive(Debug)]
pub enum GitSdkError {
    Unsupported(&'static str),
    InvalidInput(String),
    Io(std::io::Error),
    Git(String),
}

impl fmt::Display for GitSdkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(message) => write!(f, "unsupported: {message}"),
            Self::InvalidInput(message) => write!(f, "invalid input: {message}"),
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::Git(message) => write!(f, "git error: {message}"),
        }
    }
}

impl std::error::Error for GitSdkError {}

impl From<std::io::Error> for GitSdkError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Debug, Clone)]
pub struct AuthorInfo {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub id: String,
    pub message: String,
    pub author: AuthorInfo,
    pub timestamp: i64,
}

#[derive(Debug, Clone)]
pub struct CommitRange {
    pub start: String,
    pub end: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Untracked,
    Conflicted,
    Clean,
}

#[derive(Debug, Clone)]
pub struct StatusEntry {
    pub path: String,
    pub status: StatusKind,
}

pub trait GitSdk {
    fn init(&self, path: &Path) -> GitSdkResult<()>;
    fn add(&self, repo_path: &Path, paths: &[String]) -> GitSdkResult<()>;
    fn status(&self, repo_path: &Path) -> GitSdkResult<Vec<StatusEntry>>;
    fn commit(&self, repo_path: &Path, message: &str, author: &AuthorInfo) -> GitSdkResult<String>;
    fn log(&self, repo_path: &Path, max: usize) -> GitSdkResult<Vec<CommitInfo>>;
    fn branch(&self, repo_path: &Path, name: &str, target: Option<&str>) -> GitSdkResult<()>;
    fn checkout(&self, repo_path: &Path, reference: &str) -> GitSdkResult<()>;
    fn squash(
        &self,
        repo_path: &Path,
        range: &CommitRange,
        message: &str,
        author: &AuthorInfo,
    ) -> GitSdkResult<String>;
    fn diff(&self, repo_path: &Path, from: Option<&str>, to: Option<&str>) -> GitSdkResult<String>;
    fn apply_patch(&self, repo_path: &Path, patch: &str) -> GitSdkResult<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DummyGitSdk;

impl GitSdk for DummyGitSdk {
    fn init(&self, _path: &Path) -> GitSdkResult<()> {
        Ok(())
    }

    fn add(&self, _repo_path: &Path, _paths: &[String]) -> GitSdkResult<()> {
        Ok(())
    }

    fn status(&self, _repo_path: &Path) -> GitSdkResult<Vec<StatusEntry>> {
        Ok(Vec::new())
    }

    fn commit(
        &self,
        _repo_path: &Path,
        _message: &str,
        _author: &AuthorInfo,
    ) -> GitSdkResult<String> {
        Ok("dummy".to_string())
    }

    fn log(&self, _repo_path: &Path, _max: usize) -> GitSdkResult<Vec<CommitInfo>> {
        Ok(Vec::new())
    }

    fn branch(&self, _repo_path: &Path, _name: &str, _target: Option<&str>) -> GitSdkResult<()> {
        Ok(())
    }

    fn checkout(&self, _repo_path: &Path, _reference: &str) -> GitSdkResult<()> {
        Ok(())
    }

    fn squash(
        &self,
        _repo_path: &Path,
        _range: &CommitRange,
        _message: &str,
        _author: &AuthorInfo,
    ) -> GitSdkResult<String> {
        Ok("dummy".to_string())
    }

    fn diff(
        &self,
        _repo_path: &Path,
        _from: Option<&str>,
        _to: Option<&str>,
    ) -> GitSdkResult<String> {
        Ok(String::new())
    }

    fn apply_patch(&self, _repo_path: &Path, _patch: &str) -> GitSdkResult<()> {
        Ok(())
    }
}
