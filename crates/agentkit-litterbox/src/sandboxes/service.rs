use std::path::{Path, PathBuf};

use tar::Archive;
use tempfile::TempDir;

use crate::backends::{self, Selected};
use crate::config::Config;
use crate::domain::{SandboxError, ScmMode};
use crate::metadata_store::{self, LoadedSandbox, StoredSandbox};
use crate::sandbox::contract::{
    CreateRequest, Error, Execution, Inspection, RequestedPort, SandboxRecord, SandboxStatus,
    Workspace,
};
use crate::scm::{Scm, ThreadSafeScm};
use crate::vcs_store::VcsStore;

/// The path a sandbox sees its workspace at.
pub const WORKSPACE_MOUNT: &str = "/src";

/// Sandboxes in one project, coordinated across SCM, a backend and the store.
pub struct Sandboxes {
    config: Config,
    scm: ThreadSafeScm,
    project_slug: String,
}

/// What a project's sandboxes currently look like.
#[derive(Clone, Debug, Default)]
pub struct Listing {
    /// Records written by this version.
    pub sandboxes: Vec<SandboxRecord>,
    /// Records written by an earlier release, reported and never adopted.
    pub legacy: Vec<String>,
}

impl Sandboxes {
    /// Open the project the current directory belongs to.
    pub fn open(config: Config) -> Result<Self, SandboxError> {
        let project_slug = resolve_project_slug(&config);
        let mode = config.git.snapshot_mode.unwrap_or_default();
        let scm = open_scm(&config, &project_slug, mode)?;

        Ok(Self {
            config,
            scm,
            project_slug,
        })
    }

    /// The project these sandboxes belong to.
    pub fn project_slug(&self) -> &str {
        &self.project_slug
    }

    /// Create a sandbox: branch, workspace, compute and record, in that order.
    pub async fn create(&self, name: &str) -> Result<SandboxRecord, Error> {
        let selected = backends::select(&self.config).await?;

        let branch_name = self.scm.create_branch(name)?;
        let staged = match self.stage() {
            Ok(staged) => staged,
            Err(error) => {
                let _ = self.scm.delete_branch(name);
                return Err(error);
            }
        };

        let request = CreateRequest {
            name: name.to_string(),
            project_slug: self.project_slug.clone(),
            workspace: Workspace {
                source: staged.path().to_path_buf(),
                mount: WORKSPACE_MOUNT.to_string(),
            },
            setup_command: self.config.project.setup_command.clone(),
            ports: self.config.ports.iter().map(port).collect(),
        };

        let record = match selected.core.create(&request).await {
            Ok(record) => record,
            Err(error) => {
                let _ = self.scm.delete_branch(name);
                return Err(error);
            }
        };

        let stored = StoredSandbox {
            sandbox: record.clone(),
            branch_name,
            remote: self.config.git.snapshot_mode == Some(ScmMode::Remote),
        };

        if let Err(error) = metadata_store::store_sandbox(&self.project_slug, &stored) {
            let _ = selected.core.delete(&record).await;
            let _ = self.scm.delete_branch(name);
            return Err(error.into());
        }

        Ok(record)
    }

    /// Delete a sandbox: compute first, then the branch, each half idempotent.
    pub async fn delete(&self, name: &str) -> Result<(), Error> {
        let stored = self.stored(name)?;
        let selected = self.select_for(&stored.sandbox).await?;

        selected.core.delete(&stored.sandbox).await?;

        // A branch that is already gone is not a failure.
        let _ = self.scm.delete_branch(name);

        if stored.remote && self.scm.list_sandboxes().is_ok_and(|left| left.is_empty()) {
            let _ = VcsStore::destroy_bare(&self.project_slug);
        }

        metadata_store::remove_sandbox(&self.project_slug, name)?;
        Ok(())
    }

    /// Run a command in a sandbox.
    pub async fn exec(&self, name: &str, command: &[String]) -> Result<Execution, Error> {
        let stored = self.stored(name)?;
        let selected = self.select_for(&stored.sandbox).await?;
        selected.core.exec(&stored.sandbox, command).await
    }

    /// Describe a sandbox.
    pub async fn inspect(&self, name: &str) -> Result<Inspection, Error> {
        let stored = self.stored(name)?;
        let selected = self.select_for(&stored.sandbox).await?;
        selected.core.inspect(&stored.sandbox).await
    }

    /// Pause a sandbox, when its mechanism can.
    pub async fn pause(&self, name: &str) -> Result<(), Error> {
        let stored = self.stored(name)?;
        let selected = self.select_for(&stored.sandbox).await?;
        let lifecycle = selected.lifecycle.ok_or_else(|| {
            unsupported(
                "pause",
                &stored.sandbox,
                "the mechanism has no sandbox-level pause",
                "stop the command instead",
            )
        })?;
        lifecycle.pause(&stored.sandbox).await
    }

    /// Resume a paused sandbox, when its mechanism can.
    pub async fn resume(&self, name: &str) -> Result<(), Error> {
        let stored = self.stored(name)?;
        let selected = self.select_for(&stored.sandbox).await?;
        let lifecycle = selected.lifecycle.ok_or_else(|| {
            unsupported(
                "resume",
                &stored.sandbox,
                "the mechanism has no sandbox-level resume",
                "start the command again",
            )
        })?;
        lifecycle.resume(&stored.sandbox).await
    }

    /// Every sandbox in this project, current first.
    pub fn list(&self) -> Result<Listing, Error> {
        let mut listing = Listing::default();
        for name in self.scm.list_sandboxes()? {
            match metadata_store::load_sandbox(&self.project_slug, &name) {
                Ok(Some(LoadedSandbox::Current(stored))) => listing.sandboxes.push(stored.sandbox),
                Ok(Some(LoadedSandbox::Legacy { .. })) => listing.legacy.push(name),
                Ok(None) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(listing)
    }

    /// Pause every sandbox in this project, skipping any that have gone.
    pub async fn pause_listed(&self) -> Result<usize, Error> {
        let listing = self.list()?;
        let selected = backends::select(&self.config).await?;
        let lifecycle = selected.lifecycle.ok_or_else(|| Error::Unsupported {
            operation: "pause".to_string(),
            backend: backends::selected_name(&self.config).to_string(),
            reason: "the mechanism has no sandbox-level pause".to_string(),
            remedy: "stop the command instead".to_string(),
        })?;
        crate::sandboxes::bulk::pause_all(&listing.sandboxes, lifecycle.as_ref()).await
    }

    /// The status of a sandbox, for surfaces that only need that.
    pub async fn status(&self, name: &str) -> Result<SandboxStatus, Error> {
        Ok(self.inspect(name).await?.status)
    }

    fn stored(&self, name: &str) -> Result<StoredSandbox, Error> {
        match metadata_store::load_sandbox(&self.project_slug, name)? {
            Some(LoadedSandbox::Current(stored)) => Ok(stored),
            Some(LoadedSandbox::Legacy { .. }) => Err(Error::failed(format!(
                "sandbox '{name}' was created by an earlier release and is not supported; \
                 remove its record and create it again"
            ))),
            None => Err(Error::NotFound {
                name: name.to_string(),
            }),
        }
    }

    async fn select_for(&self, record: &SandboxRecord) -> Result<Selected, Error> {
        let selected = backends::selected_name(&self.config);
        if record.backend != selected {
            return Err(unsupported(
                "operate on the sandbox",
                record,
                &format!(
                    "it was created by the {} backend, not {selected}",
                    record.backend
                ),
                &format!("select the {} backend to operate on it", record.backend),
            ));
        }
        let built = backends::select(&self.config).await?;
        Ok(built)
    }

    fn stage(&self) -> Result<TempDir, Error> {
        let archive = self.scm.make_archive("HEAD")?;
        let tempdir = TempDir::new().map_err(SandboxError::Io)?;
        let mut archive = Archive::new(std::io::Cursor::new(archive));
        archive.unpack(tempdir.path()).map_err(SandboxError::Io)?;
        Ok(tempdir)
    }
}

fn unsupported(operation: &str, record: &SandboxRecord, reason: &str, remedy: &str) -> Error {
    Error::Unsupported {
        operation: operation.to_string(),
        backend: record.backend.clone(),
        reason: reason.to_string(),
        remedy: remedy.to_string(),
    }
}

fn port(port: &crate::config::PortRequest) -> RequestedPort {
    RequestedPort {
        name: port.name.clone(),
        target: port.target,
    }
}

/// The project slug from configuration, or the directory name.
pub fn resolve_project_slug(config: &Config) -> String {
    config.project.slug.clone().unwrap_or_else(|| {
        std::env::current_dir()
            .ok()
            .as_ref()
            .and_then(|dir| dir.file_name())
            .and_then(|name| name.to_str())
            .map(crate::domain::slugify)
            .unwrap_or_else(|| "unknown".to_string())
    })
}

fn open_scm(
    config: &Config,
    project_slug: &str,
    mode: ScmMode,
) -> Result<ThreadSafeScm, SandboxError> {
    let host = Path::new(".");

    match mode {
        ScmMode::Direct => {
            ThreadSafeScm::open_with_mode_and_prefix(host, mode, config.project.slug.clone(), None)
        }
        ScmMode::Remote => {
            let bare: PathBuf = VcsStore::clone_bare(host, project_slug)?;
            VcsStore::install_remote(host, &bare)?;
            let host_abs = host.canonicalize().unwrap_or_else(|_| host.to_path_buf());
            ThreadSafeScm::open_with_mode_and_prefix(
                &bare,
                mode,
                config.project.slug.clone(),
                Some(host_abs),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_project_slug;
    use crate::config::{Config, ProjectConfig};

    #[test]
    fn a_configured_slug_wins_over_the_directory_name() {
        let config = Config {
            project: ProjectConfig {
                slug: Some("chosen".to_string()),
                setup_command: None,
            },
            ..Config::default()
        };
        assert_eq!(resolve_project_slug(&config), "chosen");
    }

    #[test]
    fn an_absent_slug_falls_back_to_the_directory_name() {
        let slug = resolve_project_slug(&Config::default());
        assert!(!slug.is_empty());
        assert!(!slug.contains('.'));
    }
}
