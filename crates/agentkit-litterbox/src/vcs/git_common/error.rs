use thiserror::Error;

use crate::domain::SandboxError;

/// A Git failure, kept with the Git concepts it belongs to.
///
/// The variants and their messages are the old `domain::ScmError`, moved here so
/// `git2` leaves the shared kernel. It converts into the neutral
/// [`SandboxError::Vcs`] at the boundary.
#[derive(Error, Debug)]
pub enum GitError {
    #[error("Git repository open failed: {source}")]
    Open {
        #[source]
        source: git2::Error,
    },
    #[error("Git branch listing failed: {source}")]
    BranchList {
        #[source]
        source: git2::Error,
    },
    #[error("Git branch creation failed: {source}")]
    BranchCreate {
        #[source]
        source: git2::Error,
    },
    #[error("Git branch deletion failed: {source}")]
    BranchDelete {
        #[source]
        source: git2::Error,
    },
    #[error("Git archive failed: {source}")]
    Archive {
        #[source]
        source: git2::Error,
    },
    #[error("Git status failed: {source}")]
    Status {
        #[source]
        source: git2::Error,
    },
    #[error("Git index add failed: {source}")]
    IndexAdd {
        #[source]
        source: git2::Error,
    },
    #[error("Git index write failed: {source}")]
    IndexWrite {
        #[source]
        source: git2::Error,
    },
    #[error("Git index write tree failed: {source}")]
    IndexWriteTree {
        #[source]
        source: git2::Error,
    },
    #[error("Git commit failed: {source}")]
    Commit {
        #[source]
        source: git2::Error,
    },
    #[error("Git signature failed: {source}")]
    Signature {
        #[source]
        source: git2::Error,
    },
    #[error("Git head failed: {source}")]
    Head {
        #[source]
        source: git2::Error,
    },
    #[error("Git reference failed: {source}")]
    Reference {
        #[source]
        source: git2::Error,
    },
    #[error("failed to apply patch: {message}")]
    ApplyPatch { message: String },
    #[error("Git clone failed: {source}")]
    Clone {
        #[source]
        source: git2::Error,
    },
}

impl From<GitError> for SandboxError {
    fn from(error: GitError) -> Self {
        SandboxError::Vcs {
            message: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_boundary_keeps_the_git_message() {
        let source = match git2::Repository::open("/nonexistent/litterbox-test") {
            Ok(_) => panic!("the path must not exist"),
            Err(error) => error,
        };
        let sandbox = SandboxError::from(GitError::Open { source });

        assert!(
            sandbox
                .to_string()
                .starts_with("SCM error: Git repository open failed:")
        );
        assert!(sandbox.to_string().contains("/nonexistent/litterbox-test"));
    }
}
