use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tempfile::TempDir;

use crate::backends;
use crate::config::Config;
use crate::domain::SandboxError;
use crate::metadata_store::{self, LoadedSandbox, StoredSandbox};
use crate::orchestrator::lock;
use crate::orchestrator::outcome::{Deletion, InspectionReport, Performed, Recovery};
use crate::provision::StepOutcome;
use crate::provision::kind::KindDescription;
use crate::provision::prepare::{self, PrepareContext, PrepareError, SystemRunner};
use crate::sandbox::contract::{
    ContributionSupport, CreateMode, CreateRequest, Error, Execution, Forward, Inspection,
    ProvisionStep, ProvisioningClass, ProvisioningFailure, RequestedPort, ResolvedStep,
    SandboxRecord, SandboxStatus, Workspace, WorkspaceBinding,
};
use crate::sandbox::selected::Selected;
use crate::vcs::{Error as VcsError, Prepared, Vcs, Workspace as VcsWorkspace};

/// How long host preparation may take before its commands are killed and
/// reaped. Preparation runs before a backend's create budget starts, so it
/// carries its own.
const HOST_PREPARE_BUDGET: Duration = Duration::from_secs(30 * 60);

/// Sandboxes in one project, coordinated across SCM, a backend and the store.
pub struct Sandboxes {
    config: Config,
    vcs: Arc<dyn Vcs>,
    remote: bool,
    binding: Option<WorkspaceBinding>,
    project_slug: String,
    records_dir: std::path::PathBuf,
    fixed: Option<Selected>,
}

/// What a project's sandboxes currently look like.
#[derive(Clone, Debug, Default)]
pub struct Listing {
    /// Records written by this version.
    pub sandboxes: Vec<SandboxRecord>,
    /// Records that cannot be read, with the reason.
    pub corrupt: Vec<String>,
    /// Branches with no record: compute the orchestrator cannot address.
    pub orphans: Vec<String>,
    /// Why branches could not be enumerated, when they could not.
    pub branch_error: Option<String>,
}

/// Whether an operation may proceed while the sandbox is paused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PausedPolicy {
    /// The operation joins or otherwise runs work; refuse it.
    Refuse,
    /// The operation does not join; proceed.
    Allow,
}

impl Sandboxes {
    /// Build an orchestrator over an explicit backend.
    pub fn with_vcs(
        config: Config,
        vcs: Arc<dyn Vcs>,
        remote: bool,
        binding: Option<WorkspaceBinding>,
    ) -> Result<Self, SandboxError> {
        let project_slug = resolve_project_slug(&config)?;
        let records_dir = metadata_store::directory(&project_slug);

        Ok(Self {
            config,
            vcs,
            remote,
            binding,
            project_slug,
            records_dir,
            fixed: None,
        })
    }

    /// The workspace binding this machine configured, if any.
    ///
    /// `None` means no section chose one, and `create` refuses rather than
    /// defaulting.
    pub fn binding(&self) -> Option<WorkspaceBinding> {
        self.binding
    }

    /// Build an orchestrator over an explicit record directory and backend,
    /// so its rollback and refusal paths can be tested without a repository
    /// or a daemon.
    #[cfg(test)]
    pub fn for_test(
        config: Config,
        vcs: Arc<dyn Vcs>,
        remote: bool,
        project_slug: String,
        records_dir: std::path::PathBuf,
        fixed: Selected,
    ) -> Self {
        Self {
            config,
            vcs,
            remote,
            // Tests model a machine that configured its workspace; a test
            // about the missing key clears it with `with_binding`.
            binding: Some(WorkspaceBinding::Copied),
            project_slug,
            records_dir,
            fixed: Some(fixed),
        }
    }

    /// Set or clear the configured binding, for tests about the refusal paths.
    #[cfg(test)]
    pub fn with_binding(mut self, binding: Option<WorkspaceBinding>) -> Self {
        self.binding = binding;
        self
    }

    /// The project these sandboxes belong to.
    pub fn project_slug(&self) -> &str {
        &self.project_slug
    }

    /// The directory this project's records live in.
    pub fn records_dir(&self) -> &Path {
        &self.records_dir
    }

    /// The binding this create will use.
    ///
    /// A missing key is a configuration error rather than a default, and a
    /// combination the VCS cannot honour is refused before anything is made.
    fn configured_binding(&self) -> Result<WorkspaceBinding, Error> {
        let Some(binding) = self.binding else {
            return Err(Error::Misconfigured {
                message: "no workspace binding is configured; set `binding = 'copied'` or \
                          `binding = 'bound'` under [vcs.git] in \
                          .agentkit/litterbox/config.local.toml"
                    .to_string(),
            });
        };

        if !self.vcs.capabilities().bindings.honours(binding) {
            return Err(Error::Misconfigured {
                message: format!(
                    "the configured workspace binding '{}' is not one this VCS can honour; \
                     set `binding = 'copied'` under [vcs.git] in \
                     .agentkit/litterbox/config.local.toml, or select a VCS that binds",
                    binding.as_str()
                ),
            });
        }

        Ok(binding)
    }

    /// Create a sandbox, or repair one whose compute is missing.
    pub async fn create(&self, name: &str) -> Result<Performed<SandboxRecord>, Error> {
        let name = self.valid_name(name)?;
        if let Some(loaded) = metadata_store::load_sandbox_at(&self.records_dir, &name)? {
            match loaded {
                LoadedSandbox::Current(stored) => {
                    let selected = self.select_for(&stored.sandbox, "create a sandbox").await?;
                    let inspection = selected.core().inspect(&stored.sandbox).await?;
                    if inspection.status == SandboxStatus::Missing {
                        let recovery = self.repair(&name, &selected).await?;
                        let stored = self.stored(&name)?;
                        return Ok(Performed::repaired(stored.sandbox, recovery));
                    }
                    return Err(Error::Conflict { name });
                }
                LoadedSandbox::Corrupt { path, reason } => {
                    return Err(Error::failed(format!(
                        "sandbox '{name}' has an unreadable record at {}: {reason}",
                        path.display()
                    )));
                }
            }
        }

        let binding = self.configured_binding()?;
        let selected = self.selected_for_operation("create a sandbox").await?;
        let provision = self.resolve_provision(&name, &selected).await?;

        tracing::info!(sandbox = %name, project = %self.project_slug, "creating the branch");
        let prepared = self.vcs.prepare(&name).map_err(contract_error)?;
        let branch_name = prepared.workspace.branch().to_string();
        let masked_paths = self.vcs.masked_paths(&prepared.workspace);

        // A copy is materialised into a temporary directory; a bound workspace
        // is the host directory the VCS owns.
        let staged = match binding {
            WorkspaceBinding::Copied => match self.materialise(&prepared) {
                Ok(staged) => Some(staged),
                Err(error) => {
                    if let Err(cleanup) = self.vcs.delete_branch(&name) {
                        tracing::warn!(sandbox = %name, %cleanup, "the branch may remain after a failed create");
                    }
                    return Err(error.into());
                }
            },
            WorkspaceBinding::Bound => None,
        };

        let workspace = match &staged {
            Some(staged) => Workspace::copied(staged.path().to_path_buf()),
            None => match self.vcs.bound_root(&prepared.workspace) {
                Ok(root) => Workspace::bound(root, masked_paths),
                Err(error) => {
                    if let Err(cleanup) = self.vcs.delete_branch(&name) {
                        tracing::warn!(sandbox = %name, %cleanup, "the branch may remain after a failed create");
                    }
                    return Err(contract_error(error));
                }
            },
        };

        let request = CreateRequest {
            name: name.to_string(),
            project_slug: self.project_slug.clone(),
            workspace,
            provision,
            forward_bind: self.config.forwarding.bind.clone(),
            ports: self.config.ports.iter().map(port).collect(),
            mode: CreateMode::Fresh,
        };

        tracing::info!(
            sandbox = %name,
            backend = %selected.name(),
            "creating the sandbox"
        );
        let record = match selected.core().create(&request).await {
            Ok(record) => record,
            Err(error) => {
                if let Err(cleanup) = self.vcs.delete_branch(&name) {
                    tracing::warn!(sandbox = %name, %cleanup, "the branch may remain after a failed create");
                }
                return Err(error);
            }
        };

        let stored = StoredSandbox {
            sandbox: record.clone(),
            branch_name,
            remote: self.remote,
            binding: Some(binding),
        };

        if let Err(error) =
            metadata_store::store_sandbox_at(&self.records_dir, &stored.sandbox.name, &stored)
        {
            if let Err(cleanup) = selected.core().delete(&record).await {
                tracing::warn!(sandbox = %name, %cleanup, "the compute may remain after a record-store failure");
            }
            if let Err(cleanup) = self.vcs.delete_branch(&name) {
                tracing::warn!(sandbox = %name, %cleanup, "the branch may remain after a record-store failure");
            }
            return Err(error);
        }

        Ok(Performed::fresh(record))
    }

    /// Resolve the provisioning sequence for a create or a repair.
    async fn resolve_provision(
        &self,
        sandbox: &str,
        selected: &Selected,
    ) -> Result<Vec<ResolvedStep>, Error> {
        let support = selected.core().contribution_support();
        let outcomes =
            match crate::provision::preflight_steps(&self.config, selected.name(), &support) {
                Ok(outcomes) => outcomes,
                Err(error) => {
                    tracing::warn!(
                        sandbox = %sandbox,
                        backend = %selected.name(),
                        class = %error.class().as_str(),
                        %error,
                        "provisioning refused before create"
                    );
                    return Err(error);
                }
            };

        let project_root = std::env::current_dir()
            .map_err(|error| Error::failed(format!("could not read the project root: {error}")))?;
        let mut provision = Vec::with_capacity(outcomes.len());
        for outcome in outcomes {
            let planned = match outcome {
                StepOutcome::Resolved(planned) => planned,
                StepOutcome::Failed(failure) => {
                    log_refusal(sandbox, selected.name(), &failure);
                    return Err(Error::Provisioning { failure });
                }
            };
            if let Some(failure) = step_refusal(
                &self.config,
                &self.project_slug,
                selected.name(),
                &planned.step,
                planned.description.requires_consent,
            ) {
                log_refusal(sandbox, selected.name(), &failure);
                return Err(Error::Provisioning {
                    failure: Box::new(failure),
                });
            }

            let step = planned.step.clone();
            let root = project_root.clone();
            let store_view = self.config.store_view;
            let deadline = Instant::now() + HOST_PREPARE_BUDGET;
            let prepared = tokio::task::spawn_blocking(move || {
                // The runner enforces the deadline per command and reaps what it
                // kills, so a timeout here would not leave the task running.
                let runner = SystemRunner::with_deadline(deadline);
                prepare::prepare(
                    &step,
                    &PrepareContext {
                        project_root: &root,
                        store_view,
                        runner: &runner,
                    },
                )
            })
            .await
            .map_err(|error| Error::failed(format!("host preparation task failed: {error}")))?
            .map_err(|error| preparation_failure(selected.name(), &planned.step, error))?;

            provision.push(ResolvedStep {
                identity: planned.step.identity.clone(),
                kind: planned.step.kind.clone(),
                placement: planned.description.placement,
                sandbox: prepared.sandbox,
                contributions: prepared.contributions,
            });
        }
        tracing::info!(
            sandbox = %sandbox,
            backend = %selected.name(),
            steps = ?provision.iter().map(ResolvedStep::identity).collect::<Vec<_>>(),
            "resolved the provisioning sequence"
        );
        Ok(provision)
    }

    /// Repair a missing sandbox through the backend's reuse mode.
    async fn repair(&self, name: &str, selected: &Selected) -> Result<Recovery, Error> {
        let _lock = lock::acquire(&self.records_dir, name).await?;
        let stored = self.stored(name)?;
        let name = stored.sandbox.name.clone();

        let inspection = selected.core().inspect(&stored.sandbox).await?;
        match inspection.status {
            SandboxStatus::Active => {
                return Ok(Recovery {
                    sandbox: name.to_string(),
                    from: SandboxStatus::Missing,
                    action: "already repaired".to_string(),
                    branch: stored.branch_name.clone(),
                });
            }
            SandboxStatus::Paused => {
                return Err(Error::Paused {
                    name: name.to_string(),
                });
            }
            SandboxStatus::Missing => {}
            other => {
                return Err(Error::failed(format!(
                    "sandbox '{name}' is not missing: {other:?}"
                )));
            }
        }

        let binding = stored.binding();
        let workspace = VcsWorkspace::from_branch(&stored.branch_name);
        let tip = match self.vcs.tip(&workspace) {
            Ok(tip) => tip,
            Err(VcsError::NotFound { .. }) => {
                return Err(Error::Unavailable {
                    backend: selected.name().to_string(),
                    reason: format!("sandbox '{name}' has no branch; its durable half is gone"),
                    remedy: format!("run `litterbox delete --force {name}` and create it again"),
                });
            }
            Err(error) => return Err(contract_error(error)),
        };

        // The record decides how the workspace is presented, so a
        // configuration change affects only new sandboxes.
        let masked_paths = self.vcs.masked_paths(&workspace);
        let staged = match binding {
            WorkspaceBinding::Copied => {
                let staged = TempDir::new().map_err(SandboxError::Io)?;
                self.vcs
                    .materialise(&workspace, &tip, staged.path())
                    .map_err(contract_error)?;
                Some(staged)
            }
            WorkspaceBinding::Bound => None,
        };
        let created = match &staged {
            Some(staged) => Workspace::copied(staged.path().to_path_buf()),
            None => {
                let root = self.vcs.bound_root(&workspace).map_err(contract_error)?;
                Workspace::bound(root, masked_paths)
            }
        };

        let provision = self.resolve_provision(&name, selected).await?;
        let request = CreateRequest {
            name: name.to_string(),
            project_slug: self.project_slug.clone(),
            workspace: created,
            provision,
            forward_bind: self.config.forwarding.bind.clone(),
            ports: self.config.ports.iter().map(port).collect(),
            mode: CreateMode::ReuseExisting,
        };

        tracing::info!(
            sandbox = %name,
            backend = %selected.name(),
            branch = %stored.branch_name,
            "repairing the sandbox"
        );
        let record = selected.core().create(&request).await?;

        // The backend returns a fresh record; keep the branch pairing.
        let updated = StoredSandbox {
            sandbox: record.clone(),
            branch_name: stored.branch_name.clone(),
            remote: stored.remote,
            binding: stored.binding,
        };
        if let Err(error) =
            metadata_store::store_sandbox_at(&self.records_dir, &updated.sandbox.name, &updated)
        {
            if let Err(stop) = selected.core().stop(&record).await {
                tracing::warn!(sandbox = %name, %stop, "the new compute could not be stopped after a record-store failure");
            }
            return Err(Error::failed(format!(
                "the repair started compute but the record could not be saved: {error}; \
                 the new compute was stopped and the workspace was left in place"
            )));
        }

        Ok(Recovery {
            sandbox: name.to_string(),
            from: SandboxStatus::Missing,
            action: "created".to_string(),
            branch: stored.branch_name.clone(),
        })
    }

    /// Establish whether a sandbox is live, refusing or repairing as needed.
    pub async fn ensure_live(
        &self,
        name: &str,
        policy: PausedPolicy,
    ) -> Result<Option<Recovery>, Error> {
        let stored = self.stored(name)?;
        let selected = self.select_for(&stored.sandbox, "use the sandbox").await?;
        let inspection = selected.core().inspect(&stored.sandbox).await?;
        match inspection.status {
            SandboxStatus::Active => Ok(None),
            SandboxStatus::Paused => match policy {
                PausedPolicy::Refuse => Err(Error::Paused {
                    name: name.to_string(),
                }),
                PausedPolicy::Allow => Ok(None),
            },
            SandboxStatus::Missing => {
                let canonical = stored.sandbox.name.clone();
                Ok(Some(self.repair(&canonical, &selected).await?))
            }
            SandboxStatus::Unknown => Err(Error::Unavailable {
                backend: selected.name().to_string(),
                reason: format!("the backend could not report the status of '{name}'"),
                remedy: "run `litterbox list` to see the backend's reason".to_string(),
            }),
            SandboxStatus::Error { reason } => Err(Error::failed(reason)),
        }
    }

    /// Delete a sandbox: compute first, then the branch, each half idempotent.
    pub async fn delete(&self, name: &str, force: bool) -> Result<Deletion, Error> {
        let name = self.valid_name(name)?;

        match metadata_store::load_sandbox_at(&self.records_dir, &name)? {
            Some(LoadedSandbox::Current(stored)) => {
                let selected = match self.select_for(&stored.sandbox, "delete the sandbox").await {
                    Ok(selected) => selected,
                    Err(error) => {
                        if !force {
                            return Err(error);
                        }
                        // Forced deletion still has to honour ownership: another
                        // backend's record is not ours to remove.
                        let selected_name = backends::selected_name(&self.config);
                        self.ensure_owned(&stored.sandbox, selected_name, "delete the sandbox")?;
                        match self.vcs.delete_branch(&stored.sandbox.name) {
                            Ok(()) => {}
                            Err(VcsError::NotFound { .. }) => {}
                            Err(error) => return Err(contract_error(error)),
                        }
                        metadata_store::remove_sandbox_at(&self.records_dir, &stored.sandbox.name)?;
                        return Ok(Deletion {
                            record_path: Some(
                                self.records_dir
                                    .join(format!("{}.toml", stored.sandbox.name)),
                            ),
                            state_paths: Vec::new(),
                            branch: stored.branch_name,
                            compute_stopped: false,
                            detail: Some(format!("the backend could not be asked: {error}")),
                        });
                    }
                };
                let inspection = selected.core().inspect(&stored.sandbox).await;
                let status = match inspection {
                    Ok(inspection) => inspection.status,
                    Err(error) => SandboxStatus::Error {
                        reason: error.to_string(),
                    },
                };
                if !matches!(status, SandboxStatus::Paused) && !force {
                    let state = match &status {
                        SandboxStatus::Active => "active",
                        SandboxStatus::Missing => "missing",
                        SandboxStatus::Unknown | SandboxStatus::Error { .. } => {
                            "in a state that cannot be determined"
                        }
                        SandboxStatus::Paused => "paused",
                    };
                    return Err(Error::Denied {
                        operation: format!("delete sandbox '{name}'"),
                        reason: format!(
                            "it is {state}; its branch '{}' may hold unmerged work. \
                             Pass --force to delete the record, workspace and branch.",
                            stored.branch_name
                        ),
                    });
                }

                tracing::info!(sandbox = %name, backend = %selected.name(), "deleting the sandbox");
                let teardown = selected.core().delete(&stored.sandbox).await?;

                // A branch that is already gone is not a failure. Any other
                // branch error keeps the record so the delete can be retried.
                match self.vcs.delete_branch(&stored.sandbox.name) {
                    Ok(()) => {}
                    Err(VcsError::NotFound { .. }) => {}
                    Err(error) => return Err(contract_error(error)),
                }

                if stored.remote
                    && let Err(error) = self.vcs.prune()
                {
                    tracing::warn!(project = %self.project_slug, %error, "the bare clone may remain");
                }

                metadata_store::remove_sandbox_at(&self.records_dir, &stored.sandbox.name)?;
                Ok(Deletion {
                    record_path: Some(
                        self.records_dir
                            .join(format!("{}.toml", stored.sandbox.name)),
                    ),
                    state_paths: teardown.state_paths,
                    branch: stored.branch_name,
                    compute_stopped: teardown.compute_stopped,
                    detail: teardown.detail,
                })
            }
            Some(LoadedSandbox::Corrupt { path, reason }) => {
                if !force {
                    return Err(Error::Denied {
                        operation: format!("delete sandbox '{name}'"),
                        reason: "its record is unreadable, so its status and compute cannot be \
                                 determined. Pass --force to remove the record, state directory \
                                 and branch."
                            .to_string(),
                    });
                }
                let (teardown, selection_error) =
                    match self.selected_for_operation("delete the sandbox").await {
                        Ok(selected) => {
                            let record = SandboxRecord {
                                name: name.clone(),
                                backend: selected.name().to_string(),
                                project_slug: self.project_slug.clone(),
                                data: toml::Value::Table(Default::default()),
                            };
                            match selected.core().delete(&record).await {
                                Ok(teardown) => (teardown, None),
                                Err(error) => (
                                    crate::sandbox::contract::Teardown::unknown(error.to_string()),
                                    Some(error.to_string()),
                                ),
                            }
                        }
                        Err(error) => (
                            crate::sandbox::contract::Teardown::unknown(error.to_string()),
                            Some(error.to_string()),
                        ),
                    };
                match self.vcs.delete_branch(&name) {
                    Ok(()) => {}
                    Err(VcsError::NotFound { .. }) => {}
                    Err(error) => return Err(contract_error(error)),
                }
                metadata_store::remove_sandbox_at(&self.records_dir, &name)?;
                Ok(Deletion {
                    record_path: Some(path),
                    state_paths: teardown.state_paths,
                    branch: format!("litterbox/{name}"),
                    compute_stopped: teardown.compute_stopped,
                    detail: teardown.detail.or(selection_error).or(Some(reason)),
                })
            }
            None => {
                // No record: the branch may still exist from a crash before the
                // record was written. Remove it; both halves already gone means a
                // retried delete, which succeeds.
                match self.vcs.delete_branch(&name) {
                    Ok(()) => {}
                    Err(VcsError::NotFound { .. }) => {}
                    Err(error) => return Err(contract_error(error)),
                }
                Ok(Deletion {
                    record_path: None,
                    state_paths: Vec::new(),
                    branch: format!("litterbox/{name}"),
                    compute_stopped: true,
                    detail: None,
                })
            }
        }
    }

    /// Run a command in a sandbox.
    pub async fn exec(
        &self,
        name: &str,
        command: &[String],
    ) -> Result<Performed<Execution>, Error> {
        let recovery = self.ensure_live(name, PausedPolicy::Refuse).await?;
        let stored = self.stored(name)?;
        let selected = self.select_for(&stored.sandbox, "run a command").await?;
        let result = selected.core().exec(&stored.sandbox, command).await;
        match &result {
            Ok(execution) => {
                tracing::info!(sandbox = %name, exit_code = execution.exit_code, "command finished");
            }
            Err(error) => {
                tracing::error!(sandbox = %name, %error, "command failed");
            }
        }
        result.map(|value| Performed { value, recovery })
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

    /// The program and leading arguments that evaluate a shell string in the
    /// sandbox.
    pub async fn shell(&self, name: &str) -> Result<Vec<String>, Error> {
        let stored = self.stored(name)?;
        let selected = self.select_for(&stored.sandbox, "run a command").await?;
        selected.core().shell(&stored.sandbox)
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

    /// Every sandbox in this project, records first.
    pub fn list(&self) -> Result<Listing, Error> {
        let mut listing = Listing::default();
        let mut seen = BTreeSet::new();

        for entry in metadata_store::entries_in(&self.records_dir) {
            seen.insert(entry.name.clone());
            match entry.loaded {
                Ok(LoadedSandbox::Current(stored)) => {
                    listing.sandboxes.push(stored.sandbox);
                }
                Ok(LoadedSandbox::Corrupt { path, reason }) => {
                    listing
                        .corrupt
                        .push(format!("{}: {reason} ({})", entry.name, path.display()));
                }
                Err(error) => listing.corrupt.push(format!("{}: {error}", entry.name)),
            }
        }

        match self.vcs.sandboxes() {
            Ok(branches) => {
                for branch in branches {
                    if !seen.contains(&branch) {
                        listing.orphans.push(branch);
                    }
                }
            }
            Err(error) => listing.branch_error = Some(error.to_string()),
        }

        listing
            .sandboxes
            .sort_by(|left, right| left.name.cmp(&right.name));
        listing.corrupt.sort();
        listing.orphans.sort();
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
    pub async fn inspect_all(&self) -> Result<Vec<(String, InspectionReport)>, Error> {
        self.inspect_listing(self.list()?).await
    }

    /// Inspect an enumeration the caller already made.
    pub async fn inspect_listing(
        &self,
        listing: Listing,
    ) -> Result<Vec<(String, InspectionReport)>, Error> {
        let selected = match self.selected_for_operation("list").await {
            Ok(selected) => selected,
            Err(error) => {
                // The backend cannot be asked, but the records can still be
                // reported: every sandbox is unknown for the same reason.
                let backend = self
                    .fixed
                    .as_ref()
                    .map(|selected| selected.name().to_string())
                    .or_else(|| self.config.backend.clone())
                    .unwrap_or_else(|| "selected backend".to_string());
                let reason = error.to_string();
                return Ok(listing
                    .sandboxes
                    .into_iter()
                    .map(|record| {
                        (
                            record.name.clone(),
                            InspectionReport::Status {
                                name: record.name,
                                backend: backend.clone(),
                                status: SandboxStatus::Unknown,
                                reason: reason.clone(),
                            },
                        )
                    })
                    .collect());
            }
        };
        let inspections = futures_util::stream::iter(listing.sandboxes.into_iter().map(|record| {
            let selected = &selected;
            async move {
                if let Err(error) = self.ensure_owned(&record, selected.name(), "list") {
                    return (
                        record.name.clone(),
                        InspectionReport::Status {
                            name: record.name,
                            backend: selected.name().to_string(),
                            status: SandboxStatus::Error {
                                reason: error.to_string(),
                            },
                            reason: error.to_string(),
                        },
                    );
                }
                match selected.core().inspect(&record).await {
                    Ok(inspection) => (record.name, InspectionReport::Inspected(inspection)),
                    Err(error) => (
                        record.name.clone(),
                        InspectionReport::Status {
                            name: record.name,
                            backend: selected.name().to_string(),
                            status: SandboxStatus::Error {
                                reason: error.to_string(),
                            },
                            reason: error.to_string(),
                        },
                    ),
                }
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

    /// The selected backend, revalidated for every operation.
    async fn selected_for_operation(&self, operation: &str) -> Result<Selected, Error> {
        if let Some(fixed) = &self.fixed {
            return Ok(fixed.clone());
        }
        // Readiness is checked before every operation so a backend that
        // disappeared since the last call is not reused from the cache.
        backends::select_as(&self.config, operation).await
    }

    fn materialise(&self, prepared: &Prepared) -> Result<TempDir, SandboxError> {
        let staged = TempDir::new().map_err(SandboxError::Io)?;
        self.vcs
            .materialise(&prepared.workspace, &prepared.base, staged.path())?;
        Ok(staged)
    }

    /// Attach the project's store to the checkout as a remote.
    ///
    /// Returns the store path and whether the remote was added; false means the
    /// name already points at the store.
    pub fn expose_remote(&self, name: &str) -> Result<(std::path::PathBuf, bool), Error> {
        let Some(store) = self.vcs.state_path() else {
            return Err(Error::failed("this VCS has no sandbox store"));
        };
        if !store.is_dir() {
            return Err(Error::failed(format!(
                "this project has no sandbox store at '{}' yet; create a sandbox first",
                store.display()
            )));
        }
        let added = self.vcs.expose(name).map_err(contract_error)?;
        Ok((store, added))
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
    Ok(format!("{}-{}", base_slug(config)?, namespace_suffix()))
}

/// Why a step that runs project code is refused when the project has no consent.
const STEP_REASON: &str = "this project defines a provisioning step that runs or evaluates \
     project code; add its namespace to allowed-projects, or set allow-project-setup = true, in \
     ~/.config/agentkit/litterbox/config.toml to let it run";

/// What the user can do about a refused step.
const STEP_REMEDY: &str = "grant setup consent in \
     ~/.config/agentkit/litterbox/config.toml";

/// The structured refusal for a step this project may not run.
fn step_refusal(
    config: &Config,
    project_slug: &str,
    backend: &str,
    step: &ProvisionStep,
    requires_consent: bool,
) -> Option<ProvisioningFailure> {
    if !requires_consent || config.command_steps_allowed(project_slug) {
        return None;
    }
    Some(ProvisioningFailure::new(
        ProvisioningClass::Denied,
        Some(backend.to_string()),
        Some(step.identity.clone()),
        Some(step.kind.clone()),
        STEP_REASON,
        format!("{STEP_REMEDY}, adding '{project_slug}' to allowed-projects"),
    ))
}

/// Turn a preparation error into a structured provisioning failure.
fn failure_of(backend: &str, step: &ProvisionStep, error: PrepareError) -> ProvisioningFailure {
    ProvisioningFailure::new(
        error.class,
        Some(backend.to_string()),
        Some(step.identity.clone()),
        Some(step.kind.clone()),
        error.reason,
        error.remedy,
    )
}

/// Turn a preparation error into a contract error.
fn preparation_failure(backend: &str, step: &ProvisionStep, error: PrepareError) -> Error {
    Error::Provisioning {
        failure: Box::new(failure_of(backend, step, error)),
    }
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
        /// The generic step.
        step: ProvisionStep,
        /// The kind's placement, contribution sorts and consent requirement.
        description: KindDescription,
        /// The backend's support entries for the step's contribution sorts.
        support: Vec<ContributionSupport>,
        /// Consent refusal for a step that needs it, when one applies.
        refusal: Option<ProvisioningFailure>,
    },
    /// The step could not be resolved.
    Failed {
        /// The structured failure.
        failure: ProvisioningFailure,
    },
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
    support: &[ContributionSupport],
) -> Preflight {
    match crate::provision::preflight_steps(config, backend, support) {
        Ok(results) => {
            let mut steps = Vec::with_capacity(results.len());
            for result in results {
                match result {
                    StepOutcome::Resolved(planned) => {
                        let refusal = step_refusal(
                            config,
                            project_slug,
                            backend,
                            &planned.step,
                            planned.description.requires_consent,
                        );
                        let step_support = planned
                            .description
                            .sorts
                            .iter()
                            .filter_map(|sort| {
                                support.iter().find(|entry| entry.sort == *sort).cloned()
                            })
                            .collect();
                        steps.push(PlannedStep::Resolved {
                            step: planned.step,
                            description: planned.description,
                            support: step_support,
                            refusal,
                        });
                    }
                    StepOutcome::Failed(failure) => {
                        steps.push(PlannedStep::Failed { failure: *failure });
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

/// One host requirement in a doctor report.
#[derive(Clone, Debug)]
pub struct HostRequirement {
    /// The primitive or requirement.
    pub name: String,
    /// Whether it holds.
    pub ok: bool,
    /// What was found.
    pub detail: String,
    /// What the user can do when it does not hold.
    pub remedy: Option<String>,
}

/// What the read-only host check found for one step.
#[derive(Clone, Debug)]
pub struct HostReport {
    /// The step identity.
    pub identity: String,
    /// The installable the step names, when it has one.
    pub installable: Option<String>,
    /// The host requirements.
    pub requirements: Vec<HostRequirement>,
    /// The closure's path count, when it could be derived.
    pub closure_count: Option<usize>,
    /// Why the step cannot run here, when it cannot.
    pub failure: Option<ProvisioningFailure>,
}

/// Build the doctor's plan and its read-only host reports.
pub async fn preflight_host(
    config: &Config,
    project_slug: &str,
    backend: &str,
    support: &[ContributionSupport],
) -> (Preflight, Vec<HostReport>) {
    let plan = preflight(config, project_slug, backend, support);
    let store_view = config.store_view;
    let mut reports = Vec::new();
    for planned in &plan.steps {
        let PlannedStep::Resolved { step, refusal, .. } = planned else {
            continue;
        };
        if refusal.is_some() {
            continue;
        }
        let task_step = step.clone();
        let deadline = Instant::now() + HOST_PREPARE_BUDGET;
        let result = tokio::task::spawn_blocking(move || {
            let runner = SystemRunner::with_deadline(deadline);
            prepare::check_step(
                &task_step,
                &prepare::CheckContext {
                    store_view,
                    runner: &runner,
                },
            )
        })
        .await;

        let (requirements, installable, closure_count, failure) = match result {
            Ok(Ok(report)) => {
                let failure = if !report.requirements.ok() {
                    report
                        .requirements
                        .checks
                        .iter()
                        .find(|check| !check.ok)
                        .map(|check| {
                            ProvisioningFailure::new(
                                ProvisioningClass::Unavailable,
                                Some(backend.to_string()),
                                Some(step.identity.clone()),
                                Some(step.kind.clone()),
                                check.detail.clone(),
                                check
                                    .remedy
                                    .clone()
                                    .unwrap_or_else(|| "run litterbox doctor".to_string()),
                            )
                        })
                } else if let Some(Err(error)) = &report.closure {
                    Some(failure_of(backend, step, error.clone()))
                } else {
                    None
                };
                let requirements = report
                    .requirements
                    .checks
                    .iter()
                    .map(|check| HostRequirement {
                        name: check.name.to_string(),
                        ok: check.ok,
                        detail: check.detail.clone(),
                        remedy: check.remedy.clone(),
                    })
                    .collect();
                (
                    requirements,
                    report.installable,
                    report
                        .closure
                        .as_ref()
                        .and_then(|closure| closure.as_ref().ok())
                        .map(Vec::len),
                    failure,
                )
            }
            Ok(Err(error)) => (
                Vec::new(),
                None,
                None,
                Some(failure_of(backend, step, error)),
            ),
            Err(error) => (
                Vec::new(),
                None,
                None,
                Some(ProvisioningFailure::new(
                    ProvisioningClass::Unavailable,
                    Some(backend.to_string()),
                    Some(step.identity.clone()),
                    Some(step.kind.clone()),
                    format!("host check task failed: {error}"),
                    "retry the doctor",
                )),
            ),
        };

        reports.push(HostReport {
            identity: step.identity.clone(),
            installable,
            requirements,
            closure_count,
            failure,
        });
    }
    (plan, reports)
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
fn namespace_suffix() -> String {
    let path = std::env::current_dir()
        .and_then(|dir| dir.canonicalize())
        .map(|dir| dir.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unknown".to_string());

    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    // The low 32 bits, as every existing namespace, metadata directory and
    // consent entry was built: `08x` is a minimum width, so the full hash
    // would rename existing projects.
    format!("{:08x}", hash as u32)
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
    ) -> futures_util::future::BoxFuture<'a, Result<Performed<Execution>, Error>>;

    /// The program and leading arguments that evaluate a shell string in the
    /// sandbox.
    ///
    /// Callers ask for this rather than prefixing `sh -c` themselves: the
    /// backend is the only thing that knows what its sandbox can run.
    fn shell<'a>(
        &'a self,
        name: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<Vec<String>, Error>> {
        Box::pin(async move {
            let _ = name;
            Ok(vec!["sh".to_string(), "-c".to_string()])
        })
    }

    /// Copy a host path into the sandbox.
    fn upload<'a>(
        &'a self,
        name: &'a str,
        source: &'a std::path::Path,
        destination: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<Performed<()>, Error>>;

    /// Copy a sandbox path out to the host.
    fn download<'a>(
        &'a self,
        name: &'a str,
        source: &'a str,
        destination: &'a std::path::Path,
    ) -> futures_util::future::BoxFuture<'a, Result<Performed<()>, Error>>;

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
    ) -> futures_util::future::BoxFuture<'a, Result<Performed<Execution>, Error>> {
        Box::pin(Sandboxes::exec(self, name, command))
    }

    fn shell<'a>(
        &'a self,
        name: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<Vec<String>, Error>> {
        Box::pin(Sandboxes::shell(self, name))
    }

    fn upload<'a>(
        &'a self,
        name: &'a str,
        source: &'a std::path::Path,
        destination: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<Performed<()>, Error>> {
        Box::pin(Sandboxes::upload(self, name, source, destination))
    }

    fn download<'a>(
        &'a self,
        name: &'a str,
        source: &'a str,
        destination: &'a std::path::Path,
    ) -> futures_util::future::BoxFuture<'a, Result<Performed<()>, Error>> {
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
    ) -> Result<Performed<()>, Error> {
        let recovery = self.ensure_live(name, PausedPolicy::Allow).await?;
        let stored = self.stored(name)?;
        let selected = self
            .select_for(&stored.sandbox, "upload into the sandbox")
            .await?;
        selected
            .core()
            .upload(&stored.sandbox, source, destination)
            .await
            .map(|value| Performed { value, recovery })
    }

    /// Copy a sandbox path out to the host.
    pub async fn download(
        &self,
        name: &str,
        source: &str,
        destination: &std::path::Path,
    ) -> Result<Performed<()>, Error> {
        let recovery = self.ensure_live(name, PausedPolicy::Allow).await?;
        let stored = self.stored(name)?;
        let selected = self
            .select_for(&stored.sandbox, "download from the sandbox")
            .await?;
        selected
            .core()
            .download(&stored.sandbox, source, destination, None)
            .await
            .map(|value| Performed { value, recovery })
    }

    /// Download the workspace and commit it to the sandbox's snapshot branch.
    pub async fn snapshot(&self, name: &str, message: &str) -> Result<Performed<()>, Error> {
        let recovery = self.ensure_live(name, PausedPolicy::Allow).await?;
        let stored = self.stored(name)?;
        let selected = self
            .select_for(&stored.sandbox, "snapshot the workspace")
            .await?;

        let staging = tempfile::tempdir()
            .map_err(|error| Error::failed(format!("could not create a staging dir: {error}")))?;
        let workspace = selected.core().inspect(&stored.sandbox).await?.workspace;
        // The ignore rules travel first, so the bulk transfer can skip build
        // trees and secrets instead of copying them into host staging.
        let ignore = Some(self.vcs.ignore_files());
        selected
            .core()
            .download(&stored.sandbox, &workspace, staging.path(), ignore)
            .await?;

        let filter = self
            .vcs
            .snapshot_filter(staging.path())
            .map_err(contract_error)?;
        let selector = Some(filter);
        selected
            .core()
            .download(&stored.sandbox, &workspace, staging.path(), selector)
            .await?;

        let workspace_handle = VcsWorkspace::from_branch(&stored.branch_name);
        self.vcs
            .snapshot(&workspace_handle, staging.path(), message)
            .map_err(contract_error)?;
        Ok(Performed {
            value: (),
            recovery,
        })
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

impl From<VcsError> for SandboxError {
    fn from(error: VcsError) -> Self {
        match error {
            VcsError::Exists { name } => SandboxError::SandboxExists { name },
            VcsError::NotFound { name } => SandboxError::SandboxNotFound { name },
            VcsError::Io(source) => SandboxError::Io(source),
            other => SandboxError::Vcs {
                message: other.to_string(),
            },
        }
    }
}

fn contract_error(error: VcsError) -> Error {
    match error {
        VcsError::Exists { name } => Error::Conflict { name },
        VcsError::NotFound { name } => Error::NotFound { name },
        VcsError::Unsupported {
            operation,
            backend,
            primitive,
            reason,
            remedy,
        } => Error::Unsupported {
            operation,
            backend,
            primitive,
            reason,
            remedy,
        },
        VcsError::Io(source) => Error::failed(source.to_string()),
        VcsError::Backend { message, .. } => Error::failed(message),
    }
}

#[cfg(test)]
mod seam_tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use tempfile::TempDir;

    use super::Sandboxes;
    use crate::config::Config;
    use crate::metadata_store::{self, StoredSandbox};
    use crate::sandbox::contract::{
        BindingSupport, Error, ProvisionStep, ProvisioningClass, ResolvedStep, SandboxRecord,
        WorkspaceBinding,
    };
    use crate::sandbox::report::State;
    use crate::sandbox::selected::Selected;
    use crate::testing::{TestBackend, TestVcs};

    fn scratch() -> TempDir {
        TempDir::new().expect("tempdir")
    }

    fn selected(backend: Arc<TestBackend>) -> Selected {
        Selected::new("test", backend.clone()).with_lifecycle(backend)
    }

    fn command_step(identity: &str, run: &str) -> ProvisionStep {
        ProvisionStep {
            identity: identity.to_string(),
            kind: "command".to_string(),
            settings: [("run".to_string(), toml::Value::String(run.to_string()))].into(),
        }
    }

    fn devshell_step(identity: &str, installable: &str) -> ProvisionStep {
        ProvisionStep {
            identity: identity.to_string(),
            kind: "devshell".to_string(),
            settings: [(
                "installable".to_string(),
                toml::Value::String(installable.to_string()),
            )]
            .into(),
        }
    }

    fn orchestrator(
        dir: &Path,
        records: PathBuf,
        backend: Arc<TestBackend>,
        command: Option<String>,
    ) -> (Sandboxes, Arc<TestVcs>) {
        orchestrator_with(dir, records, backend, command, true)
    }

    fn orchestrator_with(
        _dir: &Path,
        records: PathBuf,
        backend: Arc<TestBackend>,
        command: Option<String>,
        allow_project_setup: bool,
    ) -> (Sandboxes, Arc<TestVcs>) {
        let vcs = Arc::new(TestVcs::new());
        let provision = command
            .as_ref()
            .map(|run| vec![command_step("setup", run)])
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
        let sandboxes = Sandboxes::for_test(
            config,
            vcs.clone(),
            false,
            "project".to_string(),
            records,
            selected(backend),
        );
        (sandboxes, vcs)
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
            binding: None,
        }
    }

    #[tokio::test]
    async fn a_failed_create_removes_the_branch_it_made() {
        let dir = scratch();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::failing());
        let (sandboxes, vcs) =
            orchestrator(dir.path(), records.path().to_path_buf(), backend, None);

        let error = sandboxes.create("demo").await.expect_err("create fails");
        assert!(error.to_string().contains("refused"), "{error}");

        assert!(
            vcs.deleted().contains(&"demo".to_string()),
            "the branch should have been removed: {:?}",
            vcs.deleted()
        );
    }

    #[tokio::test]
    async fn a_failed_command_step_removes_the_branch_and_names_the_step() {
        let dir = scratch();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::failing_step("setup"));
        let (sandboxes, vcs) = orchestrator_with(
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
            vcs.deleted().contains(&"demo".to_string()),
            "the branch should have been removed: {:?}",
            vcs.deleted()
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
        let dir = scratch();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let (sandboxes, vcs) =
            orchestrator(dir.path(), records.path().to_path_buf(), backend, None);

        sandboxes
            .delete("orphan", false)
            .await
            .expect("delete cleans up");

        assert!(
            vcs.deleted().contains(&"orphan".to_string()),
            "the orphan branch is removed: {:?}",
            vcs.deleted()
        );
    }

    #[tokio::test]
    async fn a_retried_delete_succeeds_when_both_halves_are_gone() {
        let dir = scratch();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let (sandboxes, _vcs) =
            orchestrator(dir.path(), records.path().to_path_buf(), backend, None);

        sandboxes
            .delete("nothing-here", false)
            .await
            .expect("nothing left is not a failure");
        sandboxes
            .delete("nothing-here", false)
            .await
            .expect("a retried delete succeeds");
    }

    #[tokio::test]
    async fn create_asks_for_the_mask_and_records_the_binding() {
        let dir = scratch();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let (sandboxes, vcs) = orchestrator(
            dir.path(),
            records.path().to_path_buf(),
            Arc::clone(&backend),
            None,
        );

        sandboxes.create("demo").await.expect("create");

        assert_eq!(vcs.masked(), vec!["litterbox/demo"]);
        assert_eq!(
            backend.requests()[0].workspace.binding,
            WorkspaceBinding::Copied
        );
        assert!(
            backend.requests()[0].workspace.masked_paths.is_empty(),
            "a copy carries the tree without VCS metadata, so it masks nothing"
        );

        let stored = metadata_store::load_sandbox_at(records.path(), "demo")
            .expect("readable")
            .expect("persisted");
        let metadata_store::LoadedSandbox::Current(stored) = stored else {
            panic!("a record written now is current");
        };
        assert_eq!(stored.binding(), WorkspaceBinding::Copied);
    }

    fn bound_orchestrator(
        records: PathBuf,
        backend: Arc<TestBackend>,
        root: PathBuf,
    ) -> (Sandboxes, Arc<TestVcs>) {
        let vcs = Arc::new(
            TestVcs::new()
                .with_bindings(BindingSupport::Bound)
                .with_bound_root(root),
        );
        let config = Config {
            project: crate::config::ProjectConfig {
                slug: Some("project".to_string()),
                backends: Some(vec!["test".to_string()]),
            },
            ..Config::default()
        };
        let sandboxes = Sandboxes::for_test(
            config,
            vcs.clone(),
            false,
            "project".to_string(),
            records,
            selected(backend),
        )
        .with_binding(Some(WorkspaceBinding::Bound));
        (sandboxes, vcs)
    }

    #[tokio::test]
    async fn a_bound_create_binds_the_host_directory_and_masks_the_metadata() {
        let records = TempDir::new().expect("records");
        let bound_root = TempDir::new().expect("bound root");
        let backend = Arc::new(TestBackend::new());
        let (sandboxes, vcs) = bound_orchestrator(
            records.path().to_path_buf(),
            Arc::clone(&backend),
            bound_root.path().to_path_buf(),
        );

        sandboxes.create("demo").await.expect("create");

        assert_eq!(vcs.bound(), vec!["litterbox/demo"]);
        assert!(
            vcs.materialised().is_empty(),
            "a bound create copies nothing into staging"
        );

        let request = backend.requests().remove(0);
        assert_eq!(request.workspace.binding, WorkspaceBinding::Bound);
        assert_eq!(request.workspace.source, bound_root.path().to_path_buf());
        assert_eq!(
            request.workspace.masked_paths,
            vec![PathBuf::from(".git"), PathBuf::from(".jj")]
        );
    }

    #[tokio::test]
    async fn a_failed_bound_create_deletes_the_branch_and_its_host_directory() {
        let records = TempDir::new().expect("records");
        let bound_root = TempDir::new().expect("bound root");
        let backend = Arc::new(TestBackend::failing());
        let (sandboxes, vcs) = bound_orchestrator(
            records.path().to_path_buf(),
            backend,
            bound_root.path().to_path_buf(),
        );

        sandboxes.create("demo").await.expect_err("create fails");

        // `delete_branch` owns releasing the host directory a bound sandbox
        // lived in, so the branch deletion is the cleanup the failure takes.
        assert!(
            vcs.deleted().contains(&"demo".to_string()),
            "the branch and its directory are released: {:?}",
            vcs.deleted()
        );
    }

    #[tokio::test]
    async fn a_bound_record_repairs_against_its_directory_and_keeps_its_binding() {
        let records = TempDir::new().expect("records");
        let bound_root = TempDir::new().expect("bound root");
        let backend = Arc::new(TestBackend::new());
        let (sandboxes, vcs) = bound_orchestrator(
            records.path().to_path_buf(),
            Arc::clone(&backend),
            bound_root.path().to_path_buf(),
        );
        sandboxes.create("demo").await.expect("create");
        backend.mark_missing("demo");

        let performed = sandboxes
            .exec("demo", &["true".to_string()])
            .await
            .expect("the command repairs the sandbox");
        assert_eq!(performed.value.exit_code, 0);
        assert!(
            vcs.materialised().is_empty(),
            "a bound repair materialises nothing"
        );
        assert_eq!(vcs.bound().len(), 2, "create and repair both ask");

        let stored = metadata_store::load_sandbox_at(records.path(), "demo")
            .expect("readable")
            .expect("persisted");
        let metadata_store::LoadedSandbox::Current(stored) = stored else {
            panic!("a record written now is current");
        };
        assert_eq!(
            stored.binding(),
            WorkspaceBinding::Bound,
            "the record keeps the binding it was created with"
        );
    }

    #[tokio::test]
    async fn create_refuses_when_no_binding_is_configured() {
        let dir = scratch();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let (sandboxes, vcs) =
            orchestrator(dir.path(), records.path().to_path_buf(), backend, None);
        let sandboxes = sandboxes.with_binding(None);

        let error = sandboxes.create("demo").await.expect_err("no binding");
        let message = error.to_string();
        assert!(message.contains("binding"), "{message}");
        assert!(message.contains("config.local.toml"), "{message}");
        assert!(
            vcs.prepared().is_empty(),
            "nothing is made when the key is missing"
        );
    }

    #[tokio::test]
    async fn bound_is_refused_when_the_vcs_only_copies() {
        let dir = scratch();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let (sandboxes, vcs) =
            orchestrator(dir.path(), records.path().to_path_buf(), backend, None);
        let sandboxes = sandboxes.with_binding(Some(WorkspaceBinding::Bound));

        let error = sandboxes.create("demo").await.expect_err("cannot bind");
        let message = error.to_string();
        assert!(message.contains("bound"), "{message}");
        assert!(message.contains("copied"), "{message}");
        assert!(
            vcs.prepared().is_empty(),
            "nothing is made when the combination is refused"
        );
    }

    #[tokio::test]
    async fn list_and_delete_do_not_need_a_configured_binding() {
        let dir = scratch();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let (sandboxes, _vcs) =
            orchestrator(dir.path(), records.path().to_path_buf(), backend, None);
        sandboxes
            .create("demo")
            .await
            .expect("the helper configures copied");

        let sandboxes = sandboxes.with_binding(None);
        assert_eq!(sandboxes.list().expect("list").sandboxes.len(), 1);
        sandboxes.delete("demo", true).await.expect("delete");
    }

    #[tokio::test]
    async fn create_persists_a_record_and_delete_removes_both_halves() {
        let dir = scratch();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let (sandboxes, vcs) =
            orchestrator(dir.path(), records.path().to_path_buf(), backend, None);

        let created = sandboxes.create("demo").await.expect("create");
        assert_eq!(created.value.name, "demo");
        assert_eq!(created.value.backend, "test");

        let stored = metadata_store::load_sandbox_at(records.path(), "demo")
            .expect("readable")
            .expect("persisted");
        let metadata_store::LoadedSandbox::Current(stored) = stored else {
            panic!("a record written now is current");
        };
        assert_eq!(stored.branch_name, "litterbox/demo");
        assert!(!stored.remote);
        assert_eq!(vcs.prepared(), vec!["demo"]);

        sandboxes.delete("demo", true).await.expect("delete");
        assert!(
            metadata_store::load_sandbox_at(records.path(), "demo")
                .expect("readable")
                .is_none(),
            "the record is gone"
        );
        assert!(
            vcs.deleted().contains(&"demo".to_string()),
            "the branch is removed: {:?}",
            vcs.deleted()
        );

        sandboxes
            .delete("demo", true)
            .await
            .expect("a second delete");
    }

    #[tokio::test]
    async fn pause_and_resume_are_observable_through_the_orchestrator() {
        let dir = scratch();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let (sandboxes, _vcs) =
            orchestrator(dir.path(), records.path().to_path_buf(), backend, None);
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
    async fn a_paused_sandbox_refuses_exec_before_the_backend() {
        let dir = scratch();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let (sandboxes, _vcs) =
            orchestrator(dir.path(), records.path().to_path_buf(), backend, None);
        sandboxes.create("demo").await.expect("create");
        sandboxes.pause("demo").await.expect("pause");

        let error = sandboxes
            .exec("demo", &["true".to_string()])
            .await
            .expect_err("paused exec is refused");
        assert!(
            matches!(error, Error::Paused { ref name } if name == "demo"),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn a_missing_sandbox_is_repaired_on_the_next_command() {
        let dir = scratch();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let (sandboxes, _vcs) = orchestrator(
            dir.path(),
            records.path().to_path_buf(),
            backend.clone(),
            None,
        );
        sandboxes.create("demo").await.expect("create");
        backend.mark_missing("demo");

        let performed = sandboxes
            .exec("demo", &["true".to_string()])
            .await
            .expect("the command repairs the sandbox");
        assert_eq!(performed.value.exit_code, 0);
        let recovery = performed.recovery.expect("the repair is reported");
        assert_eq!(recovery.sandbox, "demo");
        assert_eq!(recovery.branch, "litterbox/demo");
    }

    #[tokio::test]
    async fn bulk_pause_skips_a_record_owned_by_another_backend() {
        let dir = scratch();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let (sandboxes, _vcs) =
            orchestrator(dir.path(), records.path().to_path_buf(), backend, None);
        sandboxes.create("mine").await.expect("create");
        metadata_store::store_sandbox_at(records.path(), "theirs", &record("theirs", "other"))
            .expect("store");

        let summary = sandboxes.pause_listed().await.expect("pause");
        assert_eq!(summary.paused, 1);
        assert_eq!(summary.skipped, 1, "the foreign record is counted");
        assert!(summary.is_complete());
    }

    #[tokio::test]
    async fn a_listed_project_may_run_its_setup_command() {
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let vcs = Arc::new(TestVcs::new());
        let config = Config {
            project: crate::config::ProjectConfig {
                slug: Some("project".to_string()),
                backends: Some(vec!["test".to_string()]),
            },
            provision_definitions: vec![command_step("setup", "echo permitted")],
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
            vcs.clone(),
            false,
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
            .map(ResolvedStep::identity)
            .collect();
        assert_eq!(identities, vec!["setup"]);
    }

    #[tokio::test]
    async fn accepting_every_project_permits_its_setup_command() {
        let dir = scratch();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let (sandboxes, _vcs) = orchestrator_with(
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
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let vcs = Arc::new(TestVcs::new());
        let config = Config {
            project: crate::config::ProjectConfig {
                slug: Some("project".to_string()),
                backends: Some(vec!["other".to_string()]),
            },
            provision_definitions: vec![command_step("setup", "echo refused")],
            backend_sequences: std::collections::BTreeMap::from([(
                "test".to_string(),
                vec!["setup".to_string()],
            )]),
            ..Config::default()
        };
        let sandboxes = Sandboxes::for_test(
            config,
            vcs.clone(),
            false,
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
            vcs.prepared().is_empty(),
            "the refusal happens before a branch exists"
        );
        assert!(
            metadata_store::load_sandbox_at(records.path(), "demo")
                .expect("readable records")
                .is_none(),
            "the refusal writes no record"
        );
        assert!(backend.requests().is_empty(), "the backend was never asked");
    }

    #[tokio::test]
    async fn an_unsupported_or_unaccepted_kind_never_reaches_the_backend() {
        for (state, expected) in [
            (State::Unavailable, ProvisioningClass::Unsupported),
            (State::Degraded, ProvisioningClass::Unavailable),
        ] {
            let records = TempDir::new().expect("records");
            let backend = Arc::new(TestBackend::new().with_provision_state(state));
            let vcs = Arc::new(TestVcs::new());
            let config = Config {
                project: crate::config::ProjectConfig {
                    slug: Some("project".to_string()),
                    backends: Some(vec!["test".to_string()]),
                },
                provision_definitions: vec![devshell_step("setup", "./nix")],
                backend_sequences: std::collections::BTreeMap::from([(
                    "test".to_string(),
                    vec!["setup".to_string()],
                )]),
                allow_project_setup: true,
                ..Config::default()
            };
            let sandboxes = Sandboxes::for_test(
                config,
                vcs.clone(),
                false,
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
                vcs.prepared().is_empty(),
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
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let vcs = Arc::new(TestVcs::new());
        let config = Config {
            project: crate::config::ProjectConfig {
                slug: Some("project".to_string()),
                backends: Some(vec!["test".to_string(), "other".to_string()]),
            },
            provision_definitions: vec![
                command_step("z-first", "true"),
                command_step("a-second", "true"),
                command_step("other-only", "true"),
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
            vcs.clone(),
            false,
            "project".to_string(),
            records.path().to_path_buf(),
            selected(backend.clone()),
        );

        sandboxes.create("demo").await.expect("create");
        let requests = backend.requests();
        let identities: Vec<&str> = requests[0]
            .provision
            .iter()
            .map(ResolvedStep::identity)
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
        let dir = scratch();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let (sandboxes, _vcs) =
            orchestrator(dir.path(), records.path().to_path_buf(), backend, None);

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
        let dir = scratch();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let (sandboxes, _vcs) =
            orchestrator(dir.path(), records.path().to_path_buf(), backend, None);
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
        let dir = scratch();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let (sandboxes, _vcs) =
            orchestrator(dir.path(), records.path().to_path_buf(), backend, None);

        let stored = record("demo", "other");
        metadata_store::store_sandbox_at(records.path(), "demo", &stored).expect("store");

        let error = sandboxes.inspect("demo").await.expect_err("refused");
        let message = error.to_string();
        assert!(message.contains("other"), "{message}");
        assert!(message.contains("test"), "{message}");
    }

    #[tokio::test]
    async fn consent_matches_the_hashed_project_namespace() {
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let vcs = Arc::new(TestVcs::new());
        let mut config = Config {
            project: crate::config::ProjectConfig {
                slug: Some("project".to_string()),
                backends: Some(vec!["test".to_string()]),
            },
            provision_definitions: vec![command_step("setup", "echo permitted")],
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
            vcs.clone(),
            false,
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
    async fn create_records_prepare_and_materialise() {
        let dir = scratch();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let (sandboxes, vcs) =
            orchestrator(dir.path(), records.path().to_path_buf(), backend, None);

        sandboxes.create("demo").await.expect("create");

        assert_eq!(vcs.prepared(), vec!["demo"]);
        assert_eq!(
            vcs.materialised(),
            vec![("litterbox/demo".to_string(), "base".to_string())]
        );
    }

    #[tokio::test]
    async fn a_project_setup_command_needs_permission() {
        let dir = scratch();
        let records = TempDir::new().expect("records");
        let backend = Arc::new(TestBackend::new());
        let (sandboxes, vcs) = orchestrator_with(
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
                assert!(
                    failure
                        .reason
                        .contains("~/.config/agentkit/litterbox/config.toml"),
                    "{failure:?}"
                );
                assert!(
                    failure
                        .remedy
                        .contains("~/.config/agentkit/litterbox/config.toml"),
                    "{failure:?}"
                );
                assert!(!failure.remedy.is_empty(), "{failure:?}");
            }
            other => panic!("expected a denial, got {other:?}"),
        }
        assert!(error.to_string().contains("setup"), "{error}");
        assert!(
            vcs.prepared().is_empty(),
            "no branch is made for a refused step"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{namespace_suffix, resolve_project_slug};
    use crate::config::{Config, ProjectConfig};

    #[test]
    fn the_namespace_suffix_stays_eight_lowercase_hex_digits() {
        let suffix = namespace_suffix();
        assert_eq!(suffix.len(), 8, "{suffix}");
        assert!(
            suffix
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "{suffix}"
        );
    }

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
