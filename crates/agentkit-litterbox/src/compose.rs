//! The composition point.
//!
//! This is the only module that constructs a VCS backend. The CLI and MCP
//! server call [`open`] and hold an orchestrator; neither they nor the
//! orchestrator name a backend.

use std::path::Path;
use std::sync::Arc;

use crate::config::Config;
use crate::domain::{SandboxError, ScmMode};
use crate::orchestrator::{Sandboxes, resolve_project_slug};
use crate::vcs::git::{GitVcs, VcsStore};

/// Build the orchestrator for the project the current directory belongs to.
pub fn open(config: Config) -> Result<Sandboxes, SandboxError> {
    let project_slug = resolve_project_slug(&config)?;
    let mode = config.git.snapshot_mode.unwrap_or_default();
    let remote = mode == ScmMode::Remote;
    let vcs = open_git(&config, &project_slug, mode)?;

    Sandboxes::with_vcs(config, Arc::new(vcs), remote)
}

fn open_git(config: &Config, project_slug: &str, mode: ScmMode) -> Result<GitVcs, SandboxError> {
    let host = Path::new(".");

    let vcs = match mode {
        ScmMode::Direct => {
            GitVcs::open_with_mode_and_prefix(host, mode, config.project.slug.clone(), None)?
        }
        ScmMode::Remote => {
            let bare: std::path::PathBuf = VcsStore::clone_bare(host, project_slug)?;
            VcsStore::install_remote(host, &bare)?;
            let host_abs = host.canonicalize().unwrap_or_else(|_| host.to_path_buf());
            GitVcs::open_with_mode_and_prefix(
                &bare,
                mode,
                config.project.slug.clone(),
                Some(host_abs),
            )?
        }
    };

    Ok(vcs.set_store_slug(project_slug.to_string()))
}
