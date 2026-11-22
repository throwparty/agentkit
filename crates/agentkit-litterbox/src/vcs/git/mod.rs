//! The Git backend.
//!
//! `GitVcs` implements [`crate::vcs::Vcs`]. It owns the host repository and the
//! bare store, and resolves a branch to the repository that holds it. Anything
//! it shares with the jj bridge comes from [`crate::vcs::git_common`]; it never
//! imports `vcs::jj`.

mod config;
pub mod path;
mod refs;
pub mod remote;
mod scm;
mod store;
#[cfg(test)]
mod vcs_tests;

pub use config::GitVcsConfig;
pub use scm::{GitScm, GitVcs, Scm, ThreadSafeScm};
pub use store::VcsStore;

use std::path::Path;

use crate::domain::SandboxError;
use crate::vcs::git_common::GitError;

/// Attach this project's sandbox store to the current checkout as a remote.
///
/// Returns whether the remote was added; false means the name already points at
/// the store.
pub fn expose_store(store: &Path, name: &str) -> Result<bool, SandboxError> {
    let mut repo = git2::Repository::discover(".")
        .map_err(|source| SandboxError::from(GitError::Open { source }))?;
    remote::install_remote_in(&mut repo, name, store)
}
