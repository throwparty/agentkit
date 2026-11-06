use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use tar::Archive;
use tempfile::TempDir;

use crate::backends;
use crate::config::Config;
use crate::domain::{SandboxError, ScmMode};
use crate::metadata_store::{self, LoadedSandbox, StoredSandbox};
use crate::provision::StepOutcome;
use crate::sandbox::contract::{
    CreateRequest, Error, Execution, Forward, Inspection, ProvisionStep, ProvisionSupport,
    ProvisioningFailure, RequestedPort, SandboxRecord, SandboxStatus, Workspace,
};
use crate::sandbox::selected::Selected;
use crate::scm::{Scm, ThreadSafeScm};
use crate::vcs_store::VcsStore;

/// Sandboxes in one project, coordinated across SCM, a backend and the store.
pub struct Sandboxes {
    config: Config,
    scm: ThreadSafeScm,
    project_slug: String,
    records_dir: std::path::PathBuf,
    legacy_records_dir: Option<std::path::PathBuf>,
    fixed: Option<Selected>,
    /// The constructed backend, cleared when a daemon disappears so the next
    /// operation re-checks readiness instead of reusing a stale answer.
    selected: std::sync::Mutex<Option<Selected>>,
}

/// What a project's sandboxes currently look like.
#[derive(Clone, Debug, Default)]
pub struct Listing {
    /// Records written by this version.
    pub sandboxes: Vec<SandboxRecord>,
    /// Records written by an earlier release, reported and never adopted.
    pub legacy: Vec<String>,
    /// Records that cannot be read, with the reason.
    pub corrupt: Vec<String>,
    /// Branches with no record: compute the orchestrator cannot address.
    pub orphans: Vec<String>,
}

impl Sandboxes {
    /// Open the project the current directory belongs to.
    pub fn open(config: Config) -> Result<Self, SandboxError> {
        let project_slug = resolve_project_slug(&config)?;
        let mode = config.git.snapshot_mode.unwrap_or_default();
        let scm = open_scm(&config, &project_slug, mode)?;

        let records_dir = metadata_store::directory(&project_slug);

        // Records written before the namespace gained its path suffix live
        // under the bare slug. They are reported, never adopted.
        let legacy_records_dir = base_slug(&config)
            .ok()
            .map(|base| metadata_store::directory(&base))
            .filter(|dir| *dir != records_dir && dir.is_dir());

        if let Some(dir) = &legacy_records_dir {
            tracing::warn!(
                "records from an earlier release are present at {}; they are reported, not adopted",
                dir.display()
            );
        }

        Ok(Self {
            config,
            scm,
            project_slug,
            records_dir,
            legacy_records_dir,
            fixed: None,
            selected: std::sync::Mutex::new(None),
        })
    }

    /// Build an orchestrator over an explicit record directory and backend,
    /// so its rollback and refusal paths can be tested without a repository
    /// or a daemon.
    #[cfg(test)]
    pub fn for_test(
        config: Config,
        scm: ThreadSafeScm,
        project_slug: String,
        records_dir: std::path::PathBuf,
        fixed: Selected,
    ) -> Self {
        Self {
            config,
            scm,
            project_slug,
            records_dir,
            legacy_records_dir: None,
            fixed: Some(fixed),
            selected: std::sync::Mutex::new(None),
        }
    }

    /// The SCM this orchestrator pairs branches through.
    #[cfg(test)]
    pub fn scm(&self) -> &ThreadSafeScm {
        &self.scm
    }

    /// The project these sandboxes belong to.
    pub fn project_slug(&self) -> &str {
        &self.project_slug
    }

    /// The directory this project's records live in.
    pub fn records_dir(&self) -> &Path {
        &self.records_dir
    }

    /// Create a sandbox: branch, workspace, compute and record, in that order.
    pub async fn create(&self, name: &str) -> Result<SandboxRecord, Error> {
        let name = self.valid_name(name)?;
        let selected = self.selected_for_operation("create a sandbox").await?;
        let plan = preflight(
            &self.config,
            &self.project_slug,
            selected.name(),
            &selected.core().provision_support(),
        );
        if let Some(error) = plan.failure {
            tracing::warn!(
                sandbox = %name,
                backend = %selected.name(),
                class = %error.class().as_str(),
                %error,
                "provisioning refused before create"
            );
            return Err(error);
        }
        let mut provision = Vec::with_capacity(plan.steps.len());
        for planned in plan.steps {
            match planned {
                PlannedStep::Resolved { step, refusal, .. } => {
                    if let Some(failure) = refusal {
                        log_refusal(&name, selected.name(), &failure);
                        return Err(Error::Provisioning {
                            failure: Box::new(failure),
                        });
                    }
                    provision.push(step);
                }
                PlannedStep::Failed { failure } => {
                    log_refusal(&name, selected.name(), &failure);
                    return Err(Error::Provisioning {
                        failure: Box::new(failure),
                    });
                }
            }
        }
        tracing::info!(
            sandbox = %name,
            backend = %selected.name(),
            steps = ?provision.iter().map(ProvisionStep::identity).collect::<Vec<_>>(),
            "resolved the provisioning sequence"
        );

        tracing::info!(sandbox = %name, project = %self.project_slug, "creating the branch");
        let branch_name = self.scm.create_branch(&name)?;
        let staged = match self.stage() {
            Ok(staged) => staged,
            Err(error) => {
                if let Err(cleanup) = self.scm.delete_branch(&name) {
                    tracing::warn!(sandbox = %name, %cleanup, "the branch may remain after a failed create");
                }
                return Err(error);
            }
        };

        let request = CreateRequest {
            name: name.to_string(),
            project_slug: self.project_slug.clone(),
            workspace: Workspace {
                source: staged.path().to_path_buf(),
            },
            provision,
            forward_bind: self.config.forwarding.bind.clone(),
            ports: self.config.ports.iter().map(port).collect(),
        };

        tracing::info!(
            sandbox = %name,
            backend = %selected.name(),
            "creating the sandbox"
        );
        let record = match selected.core().create(&request).await {
            Ok(record) => record,
            Err(error) => {
                if let Err(cleanup) = self.scm.delete_branch(&name) {
                    tracing::warn!(sandbox = %name, %cleanup, "the branch may remain after a failed create");
                }
                return Err(error);
            }
        };

        let stored = StoredSandbox {
            sandbox: record.clone(),
            branch_name,
            remote: self.config.git.snapshot_mode == Some(ScmMode::Remote),
        };

        if let Err(error) =
            metadata_store::store_sandbox_at(&self.records_dir, &stored.sandbox.name, &stored)
        {
            if let Err(cleanup) = selected.core().delete(&record).await {
                tracing::warn!(sandbox = %name, %cleanup, "the compute may remain after a record-store failure");
            }
            if let Err(cleanup) = self.scm.delete_branch(&name) {
                tracing::warn!(sandbox = %name, %cleanup, "the branch may remain after a record-store failure");
            }
            return Err(error);
        }

        Ok(record)
    }

    /// Delete a sandbox: compute first, then the branch, each half idempotent.
    pub async fn delete(&self, name: &str) -> Result<(), Error> {
        let stored = match self.stored(name) {
            Ok(stored) => stored,
            // No record: the branch may still exist from a crash before the
            // record was written. Remove it; both halves already gone means a
            // retried delete, which succeeds.
            Err(Error::NotFound { name }) => {
                let slug = self.valid_name(&name)?;
                return match self.scm.delete_branch(&slug) {
                    Ok(()) => Ok(()),
                    Err(crate::domain::SandboxError::SandboxNotFound { .. }) => Ok(()),
                    Err(error) => Err(error.into()),
                };
            }
            Err(error) => return Err(error),
        };
        let selected = self
            .select_for(&stored.sandbox, "delete the sandbox")
            .await?;

        tracing::info!(sandbox = %name, backend = %selected.name(), "deleting the sandbox");
        selected.core().delete(&stored.sandbox).await?;

        // A branch that is already gone is not a failure.
        if let Err(error) = self.scm.delete_branch(&stored.sandbox.name) {
            tracing::warn!(sandbox = %name, %error, "the branch may remain after delete");
        }

        if stored.remote
            && self.scm.list_sandboxes().is_ok_and(|left| left.is_empty())
            && let Err(error) = VcsStore::destroy_bare(&self.project_slug)
        {
            tracing::warn!(project = %self.project_slug, %error, "the bare clone may remain");
        }

        metadata_store::remove_sandbox_at(&self.records_dir, &stored.sandbox.name)?;
        Ok(())
    }

    /// Run a command in a sandbox.
    pub async fn exec(&self, name: &str, command: &[String]) -> Result<Execution, Error> {
        let stored = self.stored(name)?;
        let selected = self.select_for(&stored.sandbox, "run a command").await?;
        let result = selected.core().exec(&stored.sandbox, command).await;
        match &result {
            Ok(execution) => {
                tracing::info!(sandbox = %name, exit_code = execution.exit_code, "command finished");
            }
            Err(error) => {
                tracing::error!(sandbox = %name, %error, "command failed");
                if matches!(error, Error::Unavailable { .. }) {
                    self.forget_selected();
                }
            }
        }
        result
    }

    /// Describe a sandbox.
    pub async fn inspect(&self, name: &str) -> Result<Inspection, Error> {
        let stored = self.stored(name)?;
        let selected = self
            .select_for(&stored.sandbox, "inspect the sandbox")
            .await?;
        selected.core().inspect(&stored.sandbox).await
    }

    /// The path the sandbox sees its workspace at.
    pub async fn workspace(&self, name: &str) -> Result<String, Error> {
        Ok(self.inspect(name).await?.workspace)
    }

    /// Pause a sandbox, when its mechanism can.
    pub async fn pause(&self, name: &str) -> Result<(), Error> {
        let stored = self.stored(name)?;
        let selected = self.select_for(&stored.sandbox, "pause").await?;
        let lifecycle = selected.lifecycle_or_unsupported("pause")?;
        lifecycle.pause(&stored.sandbox).await
    }

    /// Resume a paused sandbox, when its mechanism can.
    pub async fn resume(&self, name: &str) -> Result<(), Error> {
        let stored = self.stored(name)?;
        let selected = self.select_for(&stored.sandbox, "resume").await?;
        let lifecycle = selected.lifecycle_or_unsupported("resume")?;
        lifecycle.resume(&stored.sandbox).await
    }

    /// Remove a record written by an earlier release, without touching compute.
    ///
    /// Returns the file and the identifier it named, so the caller can tell the
    /// operator which compute is now unreferenced.
    pub fn discard_legacy(
        &self,
        name: &str,
    ) -> Result<(std::path::PathBuf, Option<String>), Error> {
        let name = self.valid_name(name)?;
        match metadata_store::load_sandbox_at(&self.records_dir, &name)? {
            Some(LoadedSandbox::Legacy { path, container_id }) => {
                metadata_store::remove_sandbox_at(&self.records_dir, &name)?;
                if let Err(error) = self.scm.delete_branch(&name) {
                    tracing::warn!(sandbox = %name, %error, "the branch may remain after discarding a legacy record");
                }
                Ok((path, container_id))
            }
            Some(LoadedSandbox::Current(_)) | Some(LoadedSandbox::Corrupt { .. }) => {
                Err(Error::Denied {
                    operation: format!("discard the legacy record '{name}'"),
                    reason: "it is a current record, not a legacy record".to_string(),
                })
            }
            None => Err(Error::NotFound { name }),
        }
    }

    /// Every sandbox in this project, current first.
    pub fn list(&self) -> Result<Listing, Error> {
        let mut listing = Listing::default();
        for name in self.scm.list_sandboxes()? {
            match metadata_store::load_sandbox_at(&self.records_dir, &name) {
                Ok(Some(LoadedSandbox::Current(stored))) => listing.sandboxes.push(stored.sandbox),
                Ok(Some(LoadedSandbox::Legacy { .. })) => listing.legacy.push(name),
                Ok(Some(LoadedSandbox::Corrupt { path, reason })) => {
                    listing
                        .corrupt
                        .push(format!("{name}: {reason} ({})", path.display()));
                }
                Ok(None) => listing.orphans.push(name),
                Err(error) => listing.corrupt.push(format!("{name}: {error}")),
            }
        }
        if let Some(dir) = &self.legacy_records_dir {
            for (name, loaded) in metadata_store::names_in(dir) {
                if matches!(loaded, LoadedSandbox::Legacy { .. }) && !listing.legacy.contains(&name)
                {
                    listing.legacy.push(name);
                }
            }
        }

        Ok(listing)
    }

    /// Pause every sandbox in this project, skipping any that have gone.
    pub async fn pause_listed(&self) -> Result<crate::orchestrator::PauseSummary, Error> {
        let listing = self.list()?;
        let selected = self.selected_for_operation("pause").await?;
        let lifecycle = selected.lifecycle_or_unsupported("pause")?;

        let mut summary = crate::orchestrator::PauseSummary::default();
        let mut owned = Vec::new();
        for record in listing.sandboxes {
            match self.ensure_owned(&record, selected.name(), "pause") {
                Ok(()) => owned.push(record),
                Err(error) => {
                    summary.skipped += 1;
                    tracing::warn!(
                        sandbox = %record.name,
                        %error,
                        "the sandbox belongs elsewhere and was skipped"
                    );
                }
            }
        }

        let paused = crate::orchestrator::bulk::pause_all(&owned, lifecycle.as_ref()).await;
        summary.paused = paused.paused;
        summary.skipped += paused.skipped;
        summary.failures = paused.failures;
        Ok(summary)
    }

    /// The status of a sandbox, for surfaces that only need that.
    pub async fn status(&self, name: &str) -> Result<SandboxStatus, Error> {
        Ok(self.inspect(name).await?.status)
    }

    /// Inspect every sandbox in this project, constructing the backend once.
    pub async fn inspect_all(&self) -> Result<Vec<(String, Result<Inspection, Error>)>, Error> {
        self.inspect_listing(self.list()?).await
    }

    /// Inspect an enumeration the caller already made.
    pub async fn inspect_listing(
        &self,
        listing: Listing,
    ) -> Result<Vec<(String, Result<Inspection, Error>)>, Error> {
        let selected = match self.selected_for_operation("list").await {
            Ok(selected) => selected,
            Err(error) => {
                // The backend cannot be asked, but the records can still be
                // reported: every sandbox is unknown for the same reason.
                let reason = error.to_string();
                return Ok(listing
                    .sandboxes
                    .into_iter()
                    .map(|record| (record.name, Err(Error::failed(reason.clone()))))
                    .collect());
            }
        };
        let inspections = futures_util::stream::iter(listing.sandboxes.into_iter().map(|record| {
            let selected = &selected;
            async move {
                if let Err(error) = self.ensure_owned(&record, selected.name(), "list") {
                    return (record.name.clone(), Err(error));
                }
                let inspection = selected.core().inspect(&record).await;
                (record.name, inspection)
            }
        }))
        .buffered(8)
        .collect::<Vec<_>>()
        .await;

        Ok(inspections)
    }

    fn stored(&self, name: &str) -> Result<StoredSandbox, Error> {
        let name = self.valid_name(name)?;
        match metadata_store::load_sandbox_at(&self.records_dir, &name)? {
            Some(LoadedSandbox::Current(stored)) => Ok(stored),
            Some(LoadedSandbox::Legacy { path, .. }) => Err(Error::failed(format!(
                "sandbox '{name}' has a record from an earlier release at {}; \
                 run `litterbox delete --discard-legacy {name}` and create it again",
                path.display()
            ))),
            Some(LoadedSandbox::Corrupt { path, reason }) => Err(Error::failed(format!(
                "sandbox '{name}' has an unreadable record at {}: {reason}",
                path.display()
            ))),
            None => Err(Error::NotFound {
                name: name.to_string(),
            }),
        }
    }

    /// Whether this project and the selected backend own the record.
    fn ensure_owned(
        &self,
        record: &SandboxRecord,
        backend: &str,
        operation: &str,
    ) -> Result<(), Error> {
        if record.project_slug != self.project_slug {
            return Err(Error::failed(format!(
                "sandbox '{}' belongs to project '{}', not '{}'",
                record.name, record.project_slug, self.project_slug
            )));
        }
        if record.backend != backend {
            return Err(unsupported(
                operation,
                record,
                "the backend that created the sandbox",
                &format!(
                    "it was created by the {} backend, not {backend}",
                    record.backend
                ),
                &format!("select the {} backend to operate on it", record.backend),
            ));
        }
        Ok(())
    }

    async fn select_for(&self, record: &SandboxRecord, operation: &str) -> Result<Selected, Error> {
        if let Some(fixed) = &self.fixed {
            self.ensure_owned(record, fixed.name(), operation)?;
            return Ok(fixed.clone());
        }
        let selected = backends::selected_name(&self.config);
        self.ensure_owned(record, selected, operation)?;
        self.selected_for_operation(operation).await
    }

    /// The selected backend, constructed once and reused until it disappears.
    async fn selected_for_operation(&self, operation: &str) -> Result<Selected, Error> {
        if let Some(fixed) = &self.fixed {
            return Ok(fixed.clone());
        }
        if let Some(selected) = self.selected.lock().expect("selected backend lock").clone() {
            return Ok(selected);
        }
        let selected = backends::select_as(&self.config, operation).await?;
        *self.selected.lock().expect("selected backend lock") = Some(selected.clone());
        Ok(selected)
    }

    /// Drop the cached backend so the next operation re-checks readiness.
    fn forget_selected(&self) {
        *self.selected.lock().expect("selected backend lock") = None;
    }

    fn stage(&self) -> Result<TempDir, Error> {
        let archive = self.scm.make_archive("HEAD")?;
        let tempdir = TempDir::new().map_err(SandboxError::Io)?;
        let mut archive = Archive::new(std::io::Cursor::new(archive));
        archive.unpack(tempdir.path()).map_err(SandboxError::Io)?;
        Ok(tempdir)
    }
}

fn unsupported(
    operation: &str,
    record: &SandboxRecord,
    primitive: &str,
    reason: &str,
    remedy: &str,
) -> Error {
    Error::Unsupported {
        operation: operation.to_string(),
        backend: record.backend.clone(),
        primitive: primitive.to_string(),
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
///
/// A configured slug becomes a path component, so it is validated rather than
/// trusted: a value the slug rules would change is refused, naming the setting.
pub fn resolve_project_slug(config: &Config) -> Result<String, SandboxError> {
    let definitions = toml::to_string(&config.provision_definitions).unwrap_or_default();
    Ok(format!(
        "{}-{}",
        base_slug(config)?,
        namespace_suffix(&definitions)
    ))
}

/// Log a structured provisioning refusal with its class and step fields.
fn log_refusal(sandbox: &str, backend: &str, failure: &ProvisioningFailure) {
    tracing::warn!(
        sandbox = %sandbox,
        backend = %backend,
        class = failure.class.as_str(),
        step = failure.step.as_deref().unwrap_or(""),
        kind = failure.kind.as_deref().unwrap_or(""),
        reason = %failure.reason,
        "provisioning refused before create"
    );
}

/// One step in a preflight plan.
#[derive(Clone, Debug)]
pub enum PlannedStep {
    /// The step resolved, with an optional consent refusal.
    Resolved {
        /// The typed step.
        step: ProvisionStep,
        /// The backend's support entry for its kind.
        support: ProvisionSupport,
        /// Consent refusal for a command step, when one applies.
        refusal: Option<ProvisioningFailure>,
    },
    /// The step could not be resolved.
    Failed {
        /// The structured failure.
        failure: ProvisioningFailure,
    },
}

impl PlannedStep {
    /// The step identity, when one is known.
    pub fn identity(&self) -> &str {
        match self {
            Self::Resolved { step, .. } => step.identity(),
            Self::Failed { failure } => failure.step.as_deref().unwrap_or("unknown"),
        }
    }

    /// The step kind, when one is known.
    pub fn kind(&self) -> &str {
        match self {
            Self::Resolved { step, .. } => step.kind().as_str(),
            Self::Failed { failure } => failure.kind.as_deref().unwrap_or("unknown"),
        }
    }
}

/// A preflight plan: the steps in order plus a whole-call failure.
#[derive(Debug)]
pub struct Preflight {
    /// Each step's outcome, in the selected backend's order.
    pub steps: Vec<PlannedStep>,
    /// A configuration or compatibility failure that prevented resolution.
    pub failure: Option<Error>,
}

/// Build the shared create/doctor plan for the selected backend.
pub fn preflight(
    config: &Config,
    project_slug: &str,
    backend: &str,
    support: &[ProvisionSupport],
) -> Preflight {
    match crate::provision::preflight_steps(config, backend, support) {
        Ok(results) => {
            let mut steps = Vec::with_capacity(results.len());
            for result in results {
                match result {
                    StepOutcome::Resolved(resolved) => {
                        let refusal =
                            config.command_step_refusal(project_slug, backend, &resolved.step);
                        steps.push(PlannedStep::Resolved {
                            step: resolved.step,
                            support: resolved.support,
                            refusal,
                        });
                    }
                    StepOutcome::Failed(Error::Provisioning { failure }) => {
                        steps.push(PlannedStep::Failed { failure: *failure });
                    }
                    // A step-level error that is not a provisioning failure
                    // must fail the whole plan rather than be dropped.
                    StepOutcome::Failed(error) => {
                        return Preflight {
                            steps,
                            failure: Some(error),
                        };
                    }
                }
            }
            Preflight {
                steps,
                failure: None,
            }
        }
        Err(error) => Preflight {
            steps: Vec::new(),
            failure: Some(error),
        },
    }
}

/// The slug without the namespace suffix: what pre-change records are keyed on.
fn base_slug(config: &Config) -> Result<String, SandboxError> {
    let base = match &config.project.slug {
        Some(configured) => {
            let slug = crate::domain::slugify_name(configured)?;
            if &slug != configured {
                return Err(SandboxError::Config(format!(
                    "project.slug '{configured}' is not a valid slug; use '{slug}'"
                )));
            }
            slug
        }
        None => directory_slug(),
    };

    Ok(if base.is_empty() {
        "project".to_string()
    } else {
        base
    })
}

/// A stable, short suffix for the current repository.
fn namespace_suffix(definitions: &str) -> String {
    // Bind the namespace to the repository's remote and to the provisioning
    // definitions the user approved, so different content in the same checkout
    // does not inherit consent.
    let identity = git2::Repository::discover(".")
        .ok()
        .and_then(|repository| {
            repository
                .find_remote("origin")
                .ok()
                .and_then(|remote| remote.url().ok().map(str::to_string))
        })
        .map(|url| url.trim_end_matches(".git").to_ascii_lowercase())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .and_then(|dir| dir.canonicalize())
                .map(|dir| dir.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "unknown".to_string())
        });

    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in identity
        .as_bytes()
        .iter()
        .chain(std::iter::once(&0u8))
        .chain(definitions.as_bytes())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn directory_slug() -> String {
    std::env::current_dir()
        .ok()
        .as_ref()
        .and_then(|dir| dir.file_name())
        .and_then(|name| name.to_str())
        .map(crate::domain::slugify)
        .unwrap_or_else(|| "unknown".to_string())
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

/// The operations the file and command tools need, addressed by sandbox name.
///
/// This is the seam the MCP tools are written against: the orchestrator
/// implements it, and tests implement it without a repository or a daemon.
pub trait SandboxIo {
    /// Run a command.
    fn exec<'a>(
        &'a self,
        name: &'a str,
        command: &'a [String],
    ) -> futures_util::future::BoxFuture<'a, Result<Execution, Error>>;

    /// Copy a host path into the sandbox.
    fn upload<'a>(
        &'a self,
        name: &'a str,
        source: &'a std::path::Path,
        destination: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<(), Error>>;

    /// Copy a sandbox path out to the host.
    fn download<'a>(
        &'a self,
        name: &'a str,
        source: &'a str,
        destination: &'a std::path::Path,
    ) -> futures_util::future::BoxFuture<'a, Result<(), Error>>;

    /// The path the sandbox sees its workspace at.
    fn workspace<'a>(
        &'a self,
        name: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<String, Error>>;
}

impl SandboxIo for Sandboxes {
    fn exec<'a>(
        &'a self,
        name: &'a str,
        command: &'a [String],
    ) -> futures_util::future::BoxFuture<'a, Result<Execution, Error>> {
        Box::pin(Sandboxes::exec(self, name, command))
    }

    fn upload<'a>(
        &'a self,
        name: &'a str,
        source: &'a std::path::Path,
        destination: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<(), Error>> {
        Box::pin(Sandboxes::upload(self, name, source, destination))
    }

    fn download<'a>(
        &'a self,
        name: &'a str,
        source: &'a str,
        destination: &'a std::path::Path,
    ) -> futures_util::future::BoxFuture<'a, Result<(), Error>> {
        Box::pin(Sandboxes::download(self, name, source, destination))
    }

    fn workspace<'a>(
        &'a self,
        name: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<String, Error>> {
        Box::pin(Sandboxes::workspace(self, name))
    }
}

impl Sandboxes {
    /// The ports a sandbox forwards, or the reason its mechanism forwards none.
    pub async fn forwards(&self, name: &str) -> Result<Vec<Forward>, Error> {
        let stored = self.stored(name)?;
        let selected = self.select_for(&stored.sandbox, "forward a port").await?;
        let forwarding = selected.forwarding_or_unsupported("forward a port")?;
        forwarding.forwards(&stored.sandbox).await
    }

    /// Copy a host path into a sandbox.
    pub async fn upload(
        &self,
        name: &str,
        source: &std::path::Path,
        destination: &str,
    ) -> Result<(), Error> {
        let stored = self.stored(name)?;
        let selected = self
            .select_for(&stored.sandbox, "upload into the sandbox")
            .await?;
        selected
            .core()
            .upload(&stored.sandbox, source, destination)
            .await
    }

    /// Copy a sandbox path out to the host.
    pub async fn download(
        &self,
        name: &str,
        source: &str,
        destination: &std::path::Path,
    ) -> Result<(), Error> {
        let stored = self.stored(name)?;
        let selected = self
            .select_for(&stored.sandbox, "download from the sandbox")
            .await?;
        selected
            .core()
            .download(&stored.sandbox, source, destination)
            .await
    }

    /// Download the workspace and commit it to the sandbox's snapshot branch.
    pub async fn snapshot(&self, name: &str, message: &str) -> Result<(), Error> {
        let stored = self.stored(name)?;
        let selected = self
            .select_for(&stored.sandbox, "snapshot the workspace")
            .await?;

        let staging = tempfile::tempdir()
            .map_err(|error| Error::failed(format!("could not create a staging dir: {error}")))?;
        let workspace = selected.core().inspect(&stored.sandbox).await?.workspace;
        selected
            .core()
            .download(&stored.sandbox, &workspace, staging.path())
            .await?;

        let scm = self.snapshot_scm(&stored)?;
        scm.commit_snapshot_from_staging(staging.path(), message)?;
        Ok(())
    }

    fn snapshot_scm(&self, stored: &StoredSandbox) -> Result<ThreadSafeScm, Error> {
        match stored.remote {
            false => Ok(ThreadSafeScm::for_sandbox(
                Path::new("."),
                self.config.project.slug.clone(),
                &stored.sandbox.name,
            )?),
            true => {
                let host = Path::new(".");
                let bare = VcsStore::clone_bare(host, &self.project_slug)?;
                let host_abs = host.canonicalize().unwrap_or_else(|_| host.to_path_buf());
                let scm = ThreadSafeScm::open_with_mode_and_prefix(
                    &bare,
                    ScmMode::Remote,
                    None,
                    Some(host_abs),
                )?;
                scm.set_snapshot_branch(stored.branch_name.clone())?;
                Ok(scm)
            }
        }
    }

    fn valid_name(&self, name: &str) -> Result<String, Error> {
        crate::domain::slugify_name(name).map_err(|error| Error::failed(error.to_string()))
    }
}

/// Conversions from the pre-contract error types.
///
/// They live with the orchestrator rather than in the contract, so the contract
/// imports nothing internal.
impl From<SandboxError> for Error {
    fn from(error: SandboxError) -> Self {
        match &error {
            SandboxError::SandboxExists { name } => {
                return Self::Conflict { name: name.clone() };
            }
            SandboxError::SandboxNotFound { name } => {
                return Self::NotFound { name: name.clone() };
            }
            SandboxError::Config(message) => {
                return Self::Misconfigured {
                    message: message.clone(),
                };
            }
            _ => {}
        }

        Self::Failed {
            message: error.to_string(),
            source: Some(Box::new(error)),
        }
    }
}

#[cfg(test)]
mod seam_tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use git2::{IndexAddOption, Repository, Signature};
    use tempfile::TempDir;

    use super::Sandboxes;
    use crate::config::Config;
    use crate::metadata_store::{self, StoredSandbox};
    use crate::sandbox::contract::{Error, ProvisioningClass, SandboxRecord};
    use crate::sandbox::report::State;
    use crate::sandbox::selected::Selected;
    use crate::scm::{Scm, ThreadSafeScm};
    use crate::testing::TestBackend;

    fn repository() -> (TempDir, Repository) {
        let dir = TempDir::new().expect("tempdir");
        let repo = Repository::init(dir.path()).expect("init");
        std::fs::write(dir.path().join("README.md"), "hello").expect("write");
        let mut index = repo.index().expect("index");
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .expect("add");
        index.write().expect("write index");
        let tree_id = index.write_tree().expect("tree");
        let signature = Signature::now("Test", "test@example.com").expect("signature");
        {
            let tree = repo.find_tree(tree_id).expect("find tree");
            repo.commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])
                .expect("commit");
        }
        (dir, repo)
    }

    fn selected(backend: Arc<TestBackend>) -> Selected {
        Selected::new("test", backend.clone()).with_lifecycle(backend)
    }

    fn orchestrator(
        dir: &Path,
        records: PathBuf,
        backend: Arc<TestBackend>,
        command: Option<String>,
    ) -> Sandboxes {
        orchestrator_with(dir, records, backend, command, true)
    }

    fn orchestrator_with(
        dir: &Path,
        records: PathBuf,
        backend: Arc<TestBackend>,
        command: Option<String>,
        allow_project_setup: bool,
    ) -> Sandboxes {
        let scm = ThreadSafeScm::open(dir).expect("scm");
        let provision = command
            .as_ref()
            .map(|run| {
                vec![crate::sandbox::contract::ProvisionStep::Command {
                    identity: "setup".to_string(),
                    run: run.clone(),
                }]
            })
            .unwrap_or_default();
        let backend_sequences = command
            .as_ref()
            .map(|_| {
                std::collections::BTreeMap::from([("test".to_string(), vec!["setup".to_string()])])
            })
            .unwrap_or_default();
        let config = Config {
            project: crate::config::ProjectConfig {
                slug: Some("project".to_string()),
                backends: Some(vec!["test".to_string()]),
            },
            provision_definitions: provision,
            backend_sequences,
            allow_project_setup,
            ..Config::default()
        };
        Sandboxes::for_test(
            config,
            scm,
            "project".to_string(),
            records,
            selected(backend),
        )
    }

    fn record(name: &str, backend: &str) -> StoredSandbox {
        StoredSandbox {
            sandbox: SandboxRecord {
                name: name.to_string(),
                backend: backend.to_string(),
                project_slug: "project".to_string(),
                data: toml::Value::Table(Default::default()),
            },
            branch_name: format!("litterbox/{name}"),
            remote: false,
        }
    }

    #[tokio::test]
    async fn a_failed_create_removes_the_branch_it_made() {
        let (dir, _repo) = repository();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::failing());
        let sandboxes = orchestrator(dir.path(), records.path().to_path_buf(), backend, None);

        let error = sandboxes.create("demo").await.expect_err("create fails");
        assert!(error.to_string().contains("refused"), "{error}");

        let branches = sandboxes.scm().list_sandboxes().expect("branches");
        assert!(
            branches.is_empty(),
            "the branch should have been removed: {branches:?}"
        );
    }

    #[tokio::test]
    async fn a_failed_command_step_removes_the_branch_and_names_the_step() {
        let (dir, _repo) = repository();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::failing_step("setup"));
        let sandboxes = orchestrator_with(
            dir.path(),
            records.path().to_path_buf(),
            backend,
            Some("echo boom".to_string()),
            true,
        );

        let error = sandboxes.create("demo").await.expect_err("create fails");
        match &error {
            Error::Provisioning { failure } => {
                assert_eq!(failure.class, ProvisioningClass::Failed);
                assert_eq!(failure.step.as_deref(), Some("setup"));
                assert_eq!(failure.kind.as_deref(), Some("command"));
                assert_eq!(failure.exit_code, Some(7));
                assert!(
                    failure
                        .output
                        .as_deref()
                        .is_some_and(|output| output.contains("boom")),
                    "{failure:?}"
                );
            }
            other => panic!("expected a provisioning failure, got {other:?}"),
        }
        assert!(
            sandboxes
                .scm()
                .list_sandboxes()
                .expect("branches")
                .is_empty(),
            "the branch should have been removed"
        );
        assert!(
            metadata_store::load_sandbox_at(records.path(), "demo")
                .expect("readable")
                .is_none(),
            "no record remains"
        );
    }

    #[tokio::test]
    async fn deleting_removes_a_branch_that_lost_its_record() {
        let (dir, _repo) = repository();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let sandboxes = orchestrator(dir.path(), records.path().to_path_buf(), backend, None);

        sandboxes.scm().create_branch("orphan").expect("branch");
        sandboxes.delete("orphan").await.expect("delete cleans up");

        let branches = sandboxes.scm().list_sandboxes().expect("branches");
        assert!(!branches.contains(&"orphan".to_string()), "{branches:?}");
    }

    #[tokio::test]
    async fn a_retried_delete_succeeds_when_both_halves_are_gone() {
        let (dir, _repo) = repository();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let sandboxes = orchestrator(dir.path(), records.path().to_path_buf(), backend, None);

        sandboxes
            .delete("nothing-here")
            .await
            .expect("nothing left is not a failure");
        sandboxes
            .delete("nothing-here")
            .await
            .expect("a retried delete succeeds");
    }

    #[tokio::test]
    async fn create_persists_a_record_and_delete_removes_both_halves() {
        let (dir, _repo) = repository();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let sandboxes = orchestrator(dir.path(), records.path().to_path_buf(), backend, None);

        let created = sandboxes.create("demo").await.expect("create");
        assert_eq!(created.name, "demo");
        assert_eq!(created.backend, "test");

        let stored = metadata_store::load_sandbox_at(records.path(), "demo")
            .expect("readable")
            .expect("persisted");
        let metadata_store::LoadedSandbox::Current(stored) = stored else {
            panic!("a record written now is current");
        };
        assert_eq!(stored.branch_name, "litterbox/demo");
        assert!(!stored.remote);
        let branches = sandboxes.scm().list_sandboxes().expect("branches");
        assert!(branches.contains(&"demo".to_string()), "{branches:?}");

        sandboxes.delete("demo").await.expect("delete");
        assert!(
            metadata_store::load_sandbox_at(records.path(), "demo")
                .expect("readable")
                .is_none(),
            "the record is gone"
        );
        let branches = sandboxes.scm().list_sandboxes().expect("branches");
        assert!(!branches.contains(&"demo".to_string()), "{branches:?}");

        sandboxes.delete("demo").await.expect("a second delete");
    }

    #[tokio::test]
    async fn pause_and_resume_are_observable_through_the_orchestrator() {
        let (dir, _repo) = repository();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let sandboxes = orchestrator(dir.path(), records.path().to_path_buf(), backend, None);
        sandboxes.create("demo").await.expect("create");

        sandboxes.pause("demo").await.expect("pause");
        assert_eq!(
            sandboxes.status("demo").await.expect("status"),
            crate::sandbox::contract::SandboxStatus::Paused
        );
        sandboxes.resume("demo").await.expect("resume");
        assert_eq!(
            sandboxes.status("demo").await.expect("status"),
            crate::sandbox::contract::SandboxStatus::Active
        );
    }

    #[tokio::test]
    async fn bulk_pause_skips_a_record_owned_by_another_backend() {
        let (dir, _repo) = repository();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let sandboxes = orchestrator(dir.path(), records.path().to_path_buf(), backend, None);
        sandboxes.create("mine").await.expect("create");
        sandboxes.scm().create_branch("theirs").expect("branch");
        metadata_store::store_sandbox_at(records.path(), "theirs", &record("theirs", "other"))
            .expect("store");

        let summary = sandboxes.pause_listed().await.expect("pause");
        assert_eq!(summary.paused, 1);
        assert_eq!(summary.skipped, 1, "the foreign record is counted");
        assert!(summary.is_complete());
    }

    #[tokio::test]
    async fn a_listed_project_may_run_its_setup_command() {
        let (dir, _repo) = repository();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let scm = ThreadSafeScm::open(dir.path()).expect("scm");
        let config = Config {
            project: crate::config::ProjectConfig {
                slug: Some("project".to_string()),
                backends: Some(vec!["test".to_string()]),
            },
            provision_definitions: vec![crate::sandbox::contract::ProvisionStep::Command {
                identity: "setup".to_string(),
                run: "echo permitted".to_string(),
            }],
            backend_sequences: std::collections::BTreeMap::from([(
                "test".to_string(),
                vec!["setup".to_string()],
            )]),
            allow_project_setup: false,
            allowed_projects: vec!["project".to_string()],
            ..Config::default()
        };
        let sandboxes = Sandboxes::for_test(
            config,
            scm,
            "project".to_string(),
            records.path().to_path_buf(),
            selected(backend.clone()),
        );

        sandboxes
            .create("demo")
            .await
            .expect("the listed project is permitted");
        let requests = backend.requests();
        let identities: Vec<&str> = requests[0]
            .provision
            .iter()
            .map(crate::sandbox::contract::ProvisionStep::identity)
            .collect();
        assert_eq!(identities, vec!["setup"]);
    }

    #[tokio::test]
    async fn accepting_every_project_permits_its_setup_command() {
        let (dir, _repo) = repository();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let sandboxes = orchestrator_with(
            dir.path(),
            records.path().to_path_buf(),
            backend.clone(),
            Some("echo permitted".to_string()),
            true,
        );

        sandboxes
            .create("demo")
            .await
            .expect("the machine-wide grant permits it");
        let requests = backend.requests();
        assert_eq!(requests[0].provision.len(), 1);
        assert_eq!(requests[0].provision[0].identity(), "setup");
    }

    #[tokio::test]
    async fn a_resolver_failure_leaves_no_branch() {
        let (dir, _repo) = repository();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let scm = ThreadSafeScm::open(dir.path()).expect("scm");
        let config = Config {
            project: crate::config::ProjectConfig {
                slug: Some("project".to_string()),
                backends: Some(vec!["other".to_string()]),
            },
            provision_definitions: vec![crate::sandbox::contract::ProvisionStep::Command {
                identity: "setup".to_string(),
                run: "echo refused".to_string(),
            }],
            backend_sequences: std::collections::BTreeMap::from([(
                "test".to_string(),
                vec!["setup".to_string()],
            )]),
            ..Config::default()
        };
        let sandboxes = Sandboxes::for_test(
            config,
            scm,
            "project".to_string(),
            records.path().to_path_buf(),
            selected(backend.clone()),
        );

        let error = sandboxes.create("demo").await.expect_err("refused");
        assert!(
            matches!(
                error,
                Error::Provisioning { ref failure }
                    if failure.class
                        == crate::sandbox::contract::ProvisioningClass::Misconfigured
            ),
            "{error}"
        );
        assert!(
            sandboxes
                .scm()
                .list_sandboxes()
                .expect("branches")
                .is_empty(),
            "the refusal happens before a branch exists"
        );
        assert!(backend.requests().is_empty(), "the backend was never asked");
    }

    #[tokio::test]
    async fn an_unsupported_or_unaccepted_kind_never_reaches_the_backend() {
        for (state, expected) in [
            (State::Unavailable, ProvisioningClass::Unsupported),
            (State::Degraded, ProvisioningClass::Unavailable),
        ] {
            let (dir, _repo) = repository();
            let records = TempDir::new().expect("records");
            let backend = Arc::new(TestBackend::new().with_provision_state(state));
            let scm = ThreadSafeScm::open(dir.path()).expect("scm");
            let config = Config {
                project: crate::config::ProjectConfig {
                    slug: Some("project".to_string()),
                    backends: Some(vec!["test".to_string()]),
                },
                provision_definitions: vec![crate::sandbox::contract::ProvisionStep::Command {
                    identity: "setup".to_string(),
                    run: "echo refused".to_string(),
                }],
                backend_sequences: std::collections::BTreeMap::from([(
                    "test".to_string(),
                    vec!["setup".to_string()],
                )]),
                allow_project_setup: true,
                ..Config::default()
            };
            let sandboxes = Sandboxes::for_test(
                config,
                scm,
                "project".to_string(),
                records.path().to_path_buf(),
                selected(backend.clone()),
            );

            let error = sandboxes.create("demo").await.expect_err("refused");
            assert!(
                matches!(error, Error::Provisioning { ref failure } if failure.class == expected),
                "expected {expected:?}, got {error}"
            );
            assert!(
                sandboxes
                    .scm()
                    .list_sandboxes()
                    .expect("branches")
                    .is_empty(),
                "the refusal happens before a branch exists"
            );
            assert!(
                backend.requests().is_empty(),
                "the backend was never asked in state {state:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_request_carries_only_the_selected_backends_steps() {
        let (dir, _repo) = repository();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let scm = ThreadSafeScm::open(dir.path()).expect("scm");
        let config = Config {
            project: crate::config::ProjectConfig {
                slug: Some("project".to_string()),
                backends: Some(vec!["test".to_string(), "other".to_string()]),
            },
            provision_definitions: vec![
                crate::sandbox::contract::ProvisionStep::Command {
                    identity: "z-first".to_string(),
                    run: "true".to_string(),
                },
                crate::sandbox::contract::ProvisionStep::Command {
                    identity: "a-second".to_string(),
                    run: "true".to_string(),
                },
                crate::sandbox::contract::ProvisionStep::Command {
                    identity: "other-only".to_string(),
                    run: "true".to_string(),
                },
            ],
            backend_sequences: std::collections::BTreeMap::from([
                (
                    "test".to_string(),
                    vec!["z-first".to_string(), "a-second".to_string()],
                ),
                ("other".to_string(), vec!["other-only".to_string()]),
            ]),
            forwarding: crate::config::ForwardingConfig {
                bind: "0.0.0.0".to_string(),
            },
            ports: vec![crate::config::PortRequest {
                name: "web".to_string(),
                target: 8080,
            }],
            allow_project_setup: true,
            ..Config::default()
        };
        let sandboxes = Sandboxes::for_test(
            config,
            scm,
            "project".to_string(),
            records.path().to_path_buf(),
            selected(backend.clone()),
        );

        sandboxes.create("demo").await.expect("create");
        let requests = backend.requests();
        let identities: Vec<&str> = requests[0]
            .provision
            .iter()
            .map(crate::sandbox::contract::ProvisionStep::identity)
            .collect();
        assert_eq!(identities, vec!["z-first", "a-second"]);
        assert_eq!(requests[0].forward_bind, "0.0.0.0");
        assert_eq!(requests[0].ports.len(), 1);
        assert_eq!(requests[0].ports[0].name, "web");
        assert_eq!(requests[0].ports[0].target, 8080);
        let rendered = serde_json::to_string(&requests[0]).expect("serialises");
        assert!(!rendered.contains("other-only"), "{rendered}");
    }

    #[tokio::test]
    async fn a_record_from_another_project_is_refused_with_both_names() {
        let (dir, _repo) = repository();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let sandboxes = orchestrator(dir.path(), records.path().to_path_buf(), backend, None);

        let mut stored = record("demo", "test");
        stored.sandbox.project_slug = "another-project".to_string();
        metadata_store::store_sandbox_at(records.path(), "demo", &stored).expect("store");

        let error = sandboxes.inspect("demo").await.expect_err("refused");
        let message = error.to_string();
        assert!(message.contains("another-project"), "{message}");
        assert!(message.contains("project"), "{message}");
    }

    #[tokio::test]
    async fn a_mechanism_without_forwarding_reports_why() {
        let (dir, _repo) = repository();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let sandboxes = orchestrator(dir.path(), records.path().to_path_buf(), backend, None);
        sandboxes.create("demo").await.expect("create");

        match sandboxes.forwards("demo").await {
            Err(Error::Unsupported {
                operation,
                primitive,
                reason,
                remedy,
                ..
            }) => {
                assert_eq!(operation, "forward a port");
                assert!(!primitive.is_empty());
                assert!(!reason.is_empty());
                assert!(!remedy.is_empty());
            }
            other => panic!("expected an unsupported report, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_record_from_another_backend_is_refused_with_both_names() {
        let (dir, _repo) = repository();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let sandboxes = orchestrator(dir.path(), records.path().to_path_buf(), backend, None);

        let stored = record("demo", "other");
        metadata_store::store_sandbox_at(records.path(), "demo", &stored).expect("store");

        let error = sandboxes.inspect("demo").await.expect_err("refused");
        let message = error.to_string();
        assert!(message.contains("other"), "{message}");
        assert!(message.contains("test"), "{message}");
    }

    #[tokio::test]
    async fn discarding_a_current_record_is_denied_not_a_name_conflict() {
        let (dir, _repo) = repository();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let sandboxes = orchestrator(dir.path(), records.path().to_path_buf(), backend, None);
        sandboxes.create("demo").await.expect("create");

        let error = sandboxes
            .discard_legacy("demo")
            .expect_err("a current record is not legacy");
        match error {
            Error::Denied {
                operation, reason, ..
            } => {
                assert!(operation.contains("demo"), "{operation}");
                assert!(reason.contains("current record"), "{reason}");
            }
            other => panic!("expected a denial, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_legacy_record_is_reported_and_discarded() {
        let (dir, _repo) = repository();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let sandboxes = orchestrator(dir.path(), records.path().to_path_buf(), backend, None);

        sandboxes.scm().create_branch("demo").expect("branch");
        let legacy = "name = 'demo'\ncontainer_id = 'abc123'\n";
        std::fs::write(records.path().join("demo.toml"), legacy).expect("write");

        let listing = sandboxes.list().expect("list");
        assert_eq!(listing.legacy, vec!["demo".to_string()]);
        let error = sandboxes.inspect("demo").await.expect_err("legacy refused");
        let message = error.to_string();
        assert!(message.contains("delete --discard-legacy"), "{message}");
        assert!(message.contains("demo.toml"), "{message}");

        let (path, container) = sandboxes.discard_legacy("demo").expect("discard");
        assert_eq!(container.as_deref(), Some("abc123"));
        assert!(!path.exists(), "the record file was removed");
    }

    #[tokio::test]
    async fn consent_matches_the_hashed_project_namespace() {
        let (dir, _repo) = repository();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let scm = ThreadSafeScm::open(dir.path()).expect("scm");
        let mut config = Config {
            project: crate::config::ProjectConfig {
                slug: Some("project".to_string()),
                backends: Some(vec!["test".to_string()]),
            },
            provision_definitions: vec![crate::sandbox::contract::ProvisionStep::Command {
                identity: "setup".to_string(),
                run: "echo permitted".to_string(),
            }],
            backend_sequences: std::collections::BTreeMap::from([(
                "test".to_string(),
                vec!["setup".to_string()],
            )]),
            allow_project_setup: false,
            ..Config::default()
        };
        let slug = crate::orchestrator::resolve_project_slug(&config).expect("slug");
        assert!(slug.starts_with("project-"), "{slug}");
        config.allowed_projects = vec![slug.clone()];
        let sandboxes = Sandboxes::for_test(
            config,
            scm,
            slug,
            records.path().to_path_buf(),
            selected(backend),
        );
        sandboxes
            .create("demo")
            .await
            .expect("the hashed namespace is permitted");
    }

    #[tokio::test]
    async fn a_project_setup_command_needs_permission() {
        let (dir, _repo) = repository();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let sandboxes = orchestrator_with(
            dir.path(),
            records.path().to_path_buf(),
            backend,
            Some("echo setup".to_string()),
            false,
        );

        let error = sandboxes.create("demo").await.expect_err("refused");
        match &error {
            Error::Provisioning { failure } => {
                assert_eq!(
                    failure.class,
                    crate::sandbox::contract::ProvisioningClass::Denied
                );
                assert_eq!(failure.backend.as_deref(), Some("test"));
                assert_eq!(failure.step.as_deref(), Some("setup"));
                assert_eq!(failure.kind.as_deref(), Some("command"));
                assert!(failure.reason.contains("allowed-projects"), "{failure:?}");
                assert!(
                    failure.reason.contains("allow-project-setup"),
                    "{failure:?}"
                );
                assert!(!failure.remedy.is_empty(), "{failure:?}");
            }
            other => panic!("expected a denial, got {other:?}"),
        }
        assert!(error.to_string().contains("setup"), "{error}");
        assert!(
            sandboxes
                .scm()
                .list_sandboxes()
                .expect("branches")
                .is_empty(),
            "no branch is made for a refused step"
        );
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
                backends: None,
            },
            ..Config::default()
        };
        let slug = resolve_project_slug(&config).expect("slug");
        assert!(slug.starts_with("chosen-"), "{slug}");
        assert!(slug.len() > "chosen-".len(), "{slug}");
    }

    #[test]
    fn an_absent_slug_falls_back_to_the_directory_name() {
        let slug = resolve_project_slug(&Config::default()).expect("slug");
        assert!(!slug.is_empty());
        assert!(!slug.contains('.'));
    }

    #[test]
    fn a_configured_slug_that_is_a_path_is_refused() {
        let config = Config {
            project: ProjectConfig {
                slug: Some("../../etc".to_string()),
                backends: None,
            },
            ..Config::default()
        };
        let error = resolve_project_slug(&config).expect_err("path slug refused");
        assert!(error.to_string().contains("project.slug"), "{error}");
    }
}
