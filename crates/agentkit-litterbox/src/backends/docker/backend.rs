//! Docker as a sandboxing mechanism.
//!
//! The backend knows about containers, images and bindings. It knows nothing
//! about branches or project configuration. The project slug and sandbox name
//! are addressing: together they name the container, and the slug scopes an
//! enumeration. Everything else is a prepared tree and commands to confine.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

use super::compute;
use super::compute::{ContainerSpec, SandboxState};
use super::error::ComputeError;
use super::naming::container_name_for_slug;
use crate::sandbox::contract::{
    Backend, CreateRequest, Error, Execution, Forward, Inspection, ProvisionKind, ProvisionStep,
    ProvisionSupport, ProvisioningClass, ProvisioningFailure, SandboxRecord, SandboxStatus, Scope,
};
use crate::sandbox::environment::{
    Change, Environment, Origin, Provenance, StepProvenance, Variable,
};
use crate::sandbox::facets::{Forwarding, Lifecycle, Limits, ResourceLimits};
use crate::sandbox::report::{Capability, Check, EgressPosture, Readiness, State};

/// The backend's own name in records and reports.
pub const BACKEND: &str = "docker";

/// Where the workspace is mounted inside the container.
const WORKDIR: &str = "/src";

/// Docker's settings, typed here and nowhere else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DockerSettings {
    /// The image every sandbox created by this backend runs.
    pub image: String,
    /// Memory ceiling for each sandbox, in MiB.
    pub memory_mb: u64,
    /// CPU cores each sandbox may use.
    pub cpus: u32,
    /// Maximum tasks or processes per sandbox.
    pub pids_limit: u32,
    /// Extra capabilities granted to the sandbox.
    pub cap_add: Vec<String>,
}

impl Default for DockerSettings {
    fn default() -> Self {
        Self {
            image: String::new(),
            memory_mb: 4096,
            cpus: 2,
            pids_limit: 512,
            cap_add: Vec::new(),
        }
    }
}

/// Addressing data the Docker backend keeps in a sandbox record.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct DockerData {
    container: String,
    forwards: Vec<Forward>,
    environment: Provenance,
}

impl DockerData {
    fn from_record(record: &SandboxRecord) -> Result<Self, Error> {
        record
            .data
            .clone()
            .try_into()
            .map_err(|error: toml::de::Error| {
                Error::failed(format!(
                    "sandbox '{}' has an unreadable record: {error}",
                    record.name
                ))
            })
    }

    fn into_value(self) -> Result<toml::Value, Error> {
        toml::Value::try_from(self)
            .map_err(|error| Error::failed(format!("could not serialise the record: {error}")))
    }
}

/// Docker's implementation of the contract.
pub struct DockerBackend {
    compute: compute::DockerCompute,
    settings: DockerSettings,
}

impl DockerBackend {
    /// Build a backend from a connected client and its settings.
    pub fn new(compute: compute::DockerCompute, settings: DockerSettings) -> Self {
        Self { compute, settings }
    }

    fn container_name(&self, request: &CreateRequest) -> String {
        container_name_for_slug(&request.project_slug, &request.name)
    }

    async fn rollback(&self, container: &str) {
        if let Err(error) = self.compute.delete_container(container).await {
            tracing::warn!(container = %container, %error, "rollback could not remove the container");
        }
    }

    /// Create the container, retrying with fresh host ports when one is taken.
    async fn create_container(
        &self,
        request: &CreateRequest,
        container: &str,
    ) -> Result<(String, Vec<Forward>, Environment), Error> {
        let mut attempt = 0;
        loop {
            let (port_bindings, forwards) =
                super::forwarding::forwarded_ports(&request.ports, &request.forward_bind).await?;
            let environment = environment(&forwards);
            let spec =
                self.container_spec(request, container, port_bindings, &environment, &forwards)?;

            match self.compute.create_container(&spec).await {
                Ok(id) => return Ok((id, forwards, environment)),
                Err(error) if is_port_conflict(&error) => {
                    match retry_after_port_conflict(&error, attempt) {
                        Some(next) => {
                            attempt = next;
                            tracing::debug!(
                                sandbox = %request.name,
                                attempt,
                                "host port was taken; retrying with fresh ports"
                            );
                        }
                        None => {
                            return Err(Error::failed(format!(
                                "could not secure a host port for '{}' after {} attempts: {error}",
                                request.name, PORT_RETRIES
                            )));
                        }
                    }
                }
                Err(error) if is_name_conflict(&error) => {
                    return Err(Error::Conflict {
                        name: request.name.clone(),
                    });
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// The container definition for one attempt.
    fn container_spec(
        &self,
        request: &CreateRequest,
        container: &str,
        port_bindings: HashMap<String, Vec<super::compute::PortBindingSpec>>,
        environment: &Environment,
        forwards: &[Forward],
    ) -> Result<ContainerSpec, Error> {
        let env = environment
            .variables
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        Ok(ContainerSpec {
            name: container.to_string(),
            image: self.settings.image.clone(),
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                "tail -f /dev/null".to_string(),
            ],
            working_dir: Some(environment.workspace.clone()),
            env,
            // The labels are written before the steps run, so they carry the
            // base environment only. The completed steps travel in the record
            // create returns; a label must never claim a step that did not run.
            labels: labels_for(request, forwards, &environment.provenance)?,
            port_bindings,
            memory_bytes: Some((self.settings.memory_mb * 1024 * 1024) as i64),
            nano_cpus: Some(i64::from(self.settings.cpus) * 1_000_000_000),
            pids_limit: Some(i64::from(self.settings.pids_limit)),
            cap_add: self.settings.cap_add.clone(),
        })
    }
}

/// A step that could not be started at all.
fn step_failure(step: &ProvisionStep, error: ComputeError) -> Error {
    let identity = step.identity();
    let kind = step.kind().as_str().to_string();
    match error {
        ComputeError::Timeout { seconds } => Error::Provisioning {
            failure: Box::new(ProvisioningFailure {
                class: ProvisioningClass::Failed,
                backend: Some(BACKEND.to_string()),
                step: Some(identity.to_string()),
                kind: Some(kind),
                reason: format!(
                    "provisioning step '{identity}' exceeded its {seconds}s deadline and was \
                     rolled back"
                ),
                remedy: "shorten the step or run long work inside the sandbox on demand"
                    .to_string(),
                exit_code: None,
                output: None,
            }),
        },
        // A daemon or transport fault is a host problem, not a broken step.
        ComputeError::Connection { source } | ComputeError::ContainerExec { source } => {
            Error::Unavailable {
                backend: BACKEND.to_string(),
                reason: format!("provisioning step '{identity}' lost the daemon: {source}"),
                remedy: "start the Docker daemon and point DOCKER_HOST at it".to_string(),
            }
        }
        other => Error::Provisioning {
            failure: Box::new(ProvisioningFailure {
                class: ProvisioningClass::Failed,
                backend: Some(BACKEND.to_string()),
                step: Some(identity.to_string()),
                kind: Some(kind),
                reason: format!("provisioning step '{identity}' could not run: {other}"),
                remedy: "check the step's command".to_string(),
                exit_code: None,
                output: None,
            }),
        },
    }
}

/// A step that ran and exited non-zero.
fn command_failure(step: &ProvisionStep, exit_code: i32, output: &str) -> Error {
    let identity = step.identity();
    Error::Provisioning {
        failure: Box::new(ProvisioningFailure {
            class: ProvisioningClass::Failed,
            backend: Some(BACKEND.to_string()),
            step: Some(identity.to_string()),
            kind: Some(step.kind().as_str().to_string()),
            reason: format!("provisioning step '{identity}' failed with exit code {exit_code}"),
            remedy: "inspect the step output, fix the command, and create again".to_string(),
            exit_code: Some(exit_code),
            output: Some(bounded_output(output)),
        }),
    }
}

/// A create that outlived its fixed budget.
fn timeout_failure(step: &str, budget: Duration) -> Error {
    let named = !step.is_empty();
    let reason = if named {
        format!("create exceeded the {budget:?} provisioning budget while running '{step}'")
    } else {
        format!(
            "create exceeded the {budget:?} provisioning budget before running a provisioning \
             step"
        )
    };
    Error::Provisioning {
        failure: Box::new(ProvisioningFailure {
            class: ProvisioningClass::Failed,
            backend: Some(BACKEND.to_string()),
            step: named.then(|| step.to_string()),
            kind: named.then(|| ProvisionKind::Command.as_str().to_string()),
            reason,
            remedy: "reduce the work the provisioning steps perform, or split the create"
                .to_string(),
            exit_code: None,
            output: None,
        }),
    }
}

/// A workspace upload that outlived its transfer deadline.
fn upload_failure(budget: Duration) -> Error {
    Error::Unavailable {
        backend: BACKEND.to_string(),
        reason: format!("uploading the workspace exceeded the {budget:?} transfer deadline"),
        remedy: "reduce the workspace or check the connection to the Docker daemon".to_string(),
    }
}

/// The fixed ceiling for one create's provisioning, independent of step count.
const CREATE_BUDGET: Duration = Duration::from_secs(2 * 60 * 60);

/// Apply an ordered sequence, keeping only the steps that completed.
fn run_steps<'a, F>(
    sandbox: &'a str,
    provision: &'a [ProvisionStep],
    last_step: &'a Mutex<String>,
    mut exec: F,
) -> BoxFuture<'a, Result<Vec<StepProvenance>, Error>>
where
    F: FnMut(&'a str, &'a str) -> BoxFuture<'a, Result<Execution, ComputeError>> + Send + 'a,
{
    Box::pin(async move {
        let mut steps = Vec::with_capacity(provision.len());
        for step in provision {
            let ProvisionStep::Command { identity, run } = step;
            *last_step
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = identity.clone();
            let started = Instant::now();
            tracing::info!(
                sandbox = %sandbox,
                step = %identity,
                kind = step.kind().as_str(),
                "starting provisioning step"
            );
            let result = exec(identity.as_str(), run.as_str())
                .await
                .map_err(|error| step_failure(step, error))?;
            let elapsed_ms = started.elapsed().as_millis() as u64;
            if result.exit_code != 0 {
                let output = combined_output(&result.stdout, &result.stderr);
                tracing::warn!(
                    sandbox = %sandbox,
                    step = %identity,
                    kind = step.kind().as_str(),
                    exit_code = result.exit_code,
                    output_len = output.len(),
                    "the provisioning step failed"
                );
                return Err(command_failure(step, result.exit_code, &output));
            }
            steps.push(StepProvenance {
                identity: identity.clone(),
                kind: step.kind(),
                variables: Vec::new(),
            });
            tracing::info!(
                sandbox = %sandbox,
                step = %identity,
                kind = step.kind().as_str(),
                exit_code = result.exit_code,
                elapsed_ms,
                "provisioning step finished"
            );
        }
        Ok(steps)
    })
}

/// Rolls a container back if `create` is dropped before it finishes.
struct RollbackGuard {
    compute: compute::DockerCompute,
    container: String,
    armed: bool,
}

impl Drop for RollbackGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let compute = self.compute.clone();
        let container = self.container.clone();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(
                container = %container,
                "a cancelled create outside a runtime cannot be rolled back automatically"
            );
            return;
        };
        handle.spawn(async move {
            if let Err(error) = compute.delete_container(&container).await {
                tracing::warn!(
                    container = %container,
                    %error,
                    "a cancelled create could not roll back"
                );
            }
        });
    }
}

impl Backend for DockerBackend {
    fn create<'a>(
        &'a self,
        request: &'a CreateRequest,
    ) -> BoxFuture<'a, Result<SandboxRecord, Error>> {
        Box::pin(async move {
            if self.settings.image.is_empty() {
                return Err(Error::Misconfigured {
                    message: "backends.docker.image is not set; set it in the user configuration \
                              to the image sandboxes should run"
                        .to_string(),
                });
            }
            self.compute.ensure_image(&self.settings.image).await?;

            let container = self.container_name(request);
            let (id, forwards, mut environment) =
                self.create_container(request, &container).await?;

            let mut guard = RollbackGuard {
                compute: self.compute.clone(),
                container: id.clone(),
                armed: true,
            };

            let upload = tokio::time::timeout(
                UPLOAD_TIMEOUT,
                self.compute
                    .upload_path(&id, &request.workspace.source, WORKDIR),
            )
            .await;
            match upload {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    guard.armed = false;
                    self.rollback(&id).await;
                    return Err(error.into());
                }
                Err(_) => {
                    guard.armed = false;
                    self.rollback(&id).await;
                    return Err(upload_failure(UPLOAD_TIMEOUT));
                }
            }

            let last_step = Mutex::new(String::new());
            let budget = CREATE_BUDGET;
            let provision = run_steps(
                &request.name,
                &request.provision,
                &last_step,
                |_identity, run| {
                    let argv = vec!["sh".to_string(), "-c".to_string(), run.to_string()];
                    let compute = &self.compute;
                    let id = id.clone();
                    Box::pin(async move {
                        compute
                            .exec_provisioning(&id, &argv, Some(WORKDIR), PROVISION_OUTPUT_CAP)
                            .await
                    })
                },
            );

            let steps = match tokio::time::timeout(budget, provision).await {
                Ok(Ok(steps)) => steps,
                Ok(Err(error)) => {
                    guard.armed = false;
                    self.rollback(&id).await;
                    return Err(error);
                }
                Err(_) => {
                    guard.armed = false;
                    self.rollback(&id).await;
                    let step = last_step
                        .lock()
                        .map(|step| step.clone())
                        .unwrap_or_else(|poison| poison.into_inner().clone());
                    return Err(timeout_failure(&step, budget));
                }
            };
            environment.provenance.steps = steps;

            let data = DockerData {
                container: id,
                environment: environment.provenance,
                forwards,
            };
            let record = SandboxRecord {
                name: request.name.clone(),
                backend: BACKEND.to_string(),
                project_slug: request.project_slug.clone(),
                data: data.into_value()?,
            };
            // Disarm only once the record exists; a serialisation failure must
            // leave the guard free to remove the container.
            guard.armed = false;
            Ok(record)
        })
    }

    fn list<'a>(&'a self, scope: &'a Scope) -> BoxFuture<'a, Result<Vec<SandboxRecord>, Error>> {
        Box::pin(async move {
            let prefix = match scope {
                Scope::Repository { project_slug } => format!("litterbox-{project_slug}-"),
                Scope::Machine => "litterbox-".to_string(),
            };

            let summaries = self.compute.list(&prefix).await?;
            let mut records = Vec::new();
            for summary in summaries {
                let Some(record) = record_from_labels(summary.id, &summary.labels)? else {
                    continue;
                };
                records.push(record);
            }
            Ok(records)
        })
    }

    fn exec<'a>(
        &'a self,
        sandbox: &'a SandboxRecord,
        command: &'a [String],
    ) -> BoxFuture<'a, Result<Execution, Error>> {
        Box::pin(async move {
            let data = DockerData::from_record(sandbox)?;
            let result = match self
                .compute
                .exec(&data.container, command, Some(WORKDIR))
                .await
            {
                Ok(result) => result,
                Err(error) if is_missing(&error) => {
                    return Err(Error::NotFound {
                        name: sandbox.name.clone(),
                    });
                }
                Err(error) => return Err(error.into()),
            };
            Ok(Execution {
                exit_code: result.exit_code,
                stdout: result.stdout,
                stderr: result.stderr,
            })
        })
    }

    fn upload<'a>(
        &'a self,
        sandbox: &'a SandboxRecord,
        source: &'a Path,
        destination: &'a str,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let destination = confined(destination, sandbox)?;
            let data = DockerData::from_record(sandbox)?;
            match self
                .compute
                .upload_path(&data.container, source, &destination)
                .await
            {
                Ok(()) => {}
                Err(error) if is_missing(&error) => {
                    return Err(Error::NotFound {
                        name: sandbox.name.clone(),
                    });
                }
                Err(error) => return Err(error.into()),
            }
            Ok(())
        })
    }

    fn download<'a>(
        &'a self,
        sandbox: &'a SandboxRecord,
        source: &'a str,
        destination: &'a Path,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let source = confined(source, sandbox)?;
            let data = DockerData::from_record(sandbox)?;
            match self
                .compute
                .download_path(&data.container, &source, destination, WORKDIR)
                .await
            {
                Ok(()) => {}
                Err(error) if is_missing(&error) => {
                    return Err(Error::NotFound {
                        name: sandbox.name.clone(),
                    });
                }
                Err(error) => return Err(error.into()),
            }
            Ok(())
        })
    }

    fn inspect<'a>(
        &'a self,
        sandbox: &'a SandboxRecord,
    ) -> BoxFuture<'a, Result<Inspection, Error>> {
        Box::pin(async move {
            let data = DockerData::from_record(sandbox)?;
            let status = match self.compute.state(&data.container).await {
                Ok(SandboxState::Running) => SandboxStatus::Active,
                Ok(SandboxState::Paused) => SandboxStatus::Paused,
                Ok(SandboxState::Stopped) => SandboxStatus::Error {
                    reason: "the container has stopped".to_string(),
                },
                Ok(SandboxState::Missing) => SandboxStatus::Missing,
                Err(error) => SandboxStatus::Error {
                    reason: error.to_string(),
                },
            };

            Ok(Inspection {
                name: sandbox.name.clone(),
                backend: BACKEND.to_string(),
                status,
                // Docker's default bridge puts the container on the host's
                // route, so egress is direct rather than confined.
                egress: EgressPosture::Direct,
                workspace: WORKDIR.to_string(),
                environment: data.environment.clone(),
                forwards: data.forwards,
            })
        })
    }

    fn delete<'a>(&'a self, sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let data = DockerData::from_record(sandbox)?;
            // `delete_container` already treats a missing container as success.
            self.compute.delete_container(&data.container).await?;
            Ok(())
        })
    }

    fn readiness<'a>(&'a self) -> BoxFuture<'a, Result<Readiness, Error>> {
        Box::pin(async move {
            let daemon = match self.compute.ping().await {
                Ok(()) => Check {
                    primitive: "the Docker daemon".to_string(),
                    state: State::Available,
                    reason: None,
                    remedy: None,
                    fallback: None,
                },
                Err(error) => Check {
                    primitive: "the Docker daemon".to_string(),
                    state: State::Unavailable,
                    reason: Some(error.to_string()),
                    remedy: Some("start the daemon and point DOCKER_HOST at it".to_string()),
                    fallback: None,
                },
            };

            let image = if let Some(check) = missing_image_check(&self.settings.image) {
                check
            } else {
                match self.compute.image_present(&self.settings.image).await {
                    Ok(true) => Check {
                        primitive: format!("image {}", self.settings.image),
                        state: State::Available,
                        reason: None,
                        remedy: None,
                        fallback: None,
                    },
                    Ok(false) => Check {
                        primitive: format!("image {}", self.settings.image),
                        state: State::Degraded,
                        reason: Some(
                            "the image is not present and will be pulled on first use".to_string(),
                        ),
                        remedy: Some(
                            "pull the image ahead of time to avoid the first-create delay"
                                .to_string(),
                        ),
                        fallback: None,
                    },
                    Err(error) => Check {
                        primitive: format!("image {}", self.settings.image),
                        state: State::Unavailable,
                        reason: Some(error.to_string()),
                        remedy: Some(
                            "check the image name and that the registry is reachable".to_string(),
                        ),
                        fallback: None,
                    },
                }
            };

            Ok(Readiness {
                backend: BACKEND.to_string(),
                checks: vec![daemon, image],
            })
        })
    }

    fn capabilities<'a>(&'a self) -> BoxFuture<'a, Result<Vec<Capability>, Error>> {
        Box::pin(async move {
            // The standard facet and provisioning capabilities are derived by
            // `Selected`; this backend adds only its mechanism-specific entry.
            Ok(vec![Capability {
                operation: "proxied egress".to_string(),
                state: State::Unavailable,
                reason: Some(
                    "the sandbox shares the host network and reports direct egress".to_string(),
                ),
                remedy: Some(
                    "run a proxy inside the container or use a network-isolating backend"
                        .to_string(),
                ),
            }])
        })
    }

    fn provision_support(&self) -> Vec<ProvisionSupport> {
        vec![ProvisionSupport {
            kind: ProvisionKind::Command,
            state: State::Available,
            reason: None,
            remedy: None,
            fallback: None,
        }]
    }
}

impl Lifecycle for DockerBackend {
    fn pause<'a>(&'a self, sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let data = DockerData::from_record(sandbox)?;
            match self.compute.pause_container(&data.container).await {
                Ok(()) => Ok(()),
                Err(error) if is_missing(&error) => Err(Error::NotFound {
                    name: sandbox.name.clone(),
                }),
                Err(error) => Err(error.into()),
            }
        })
    }

    fn resume<'a>(&'a self, sandbox: &'a SandboxRecord) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let data = DockerData::from_record(sandbox)?;
            match self.compute.resume_container(&data.container).await {
                Ok(()) => Ok(()),
                Err(error) if is_missing(&error) => Err(Error::NotFound {
                    name: sandbox.name.clone(),
                }),
                Err(error) => Err(error.into()),
            }
        })
    }
}

impl Limits for DockerBackend {
    fn set_limits(
        &self,
        sandbox: &SandboxRecord,
        limits: &ResourceLimits,
    ) -> BoxFuture<'_, Result<(), Error>> {
        let data = match DockerData::from_record(sandbox) {
            Ok(data) => data,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let name = sandbox.name.clone();
        let memory_bytes = limits.memory_bytes;
        let tasks = limits.tasks;
        Box::pin(async move {
            // CPU stays at the share the sandbox was created with; the facet
            // carries memory and task bounds only.
            match self
                .compute
                .update_limits(&data.container, memory_bytes, tasks, None)
                .await
            {
                Ok(()) => Ok(()),
                Err(error) if is_missing(&error) => Err(Error::NotFound { name }),
                Err(error) => Err(error.into()),
            }
        })
    }
}

impl Forwarding for DockerBackend {
    fn forwards<'a>(
        &'a self,
        sandbox: &'a SandboxRecord,
    ) -> BoxFuture<'a, Result<Vec<Forward>, Error>> {
        Box::pin(async move {
            let data = DockerData::from_record(sandbox)?;
            Ok(data.forwards)
        })
    }
}

/// The image check that does not need a daemon.
fn missing_image_check(image: &str) -> Option<Check> {
    image.is_empty().then(|| Check {
        primitive: "the sandbox image".to_string(),
        state: State::Degraded,
        reason: Some(
            "backends.docker.image is not set; existing sandboxes can be managed".to_string(),
        ),
        remedy: Some(
            "set it in the user configuration to the image sandboxes should run".to_string(),
        ),
        fallback: None,
    })
}

/// The backend-owned environment: contents and provenance in one value.
fn environment(forwards: &[Forward]) -> Environment {
    let mut values = BTreeMap::new();
    let mut variables = Vec::new();
    for forward in forwards {
        values.insert(forward.env_var.clone(), forward.host_port.to_string());
        variables.push(Variable {
            name: forward.env_var.clone(),
            change: Change::Set,
        });
    }

    Environment {
        variables: values,
        provenance: Provenance {
            origin: Origin::Backend {
                from: "the Docker image".to_string(),
            },
            variables,
            host_paths: Vec::new(),
            steps: Vec::new(),
        },
        workspace: WORKDIR.to_string(),
    }
}

/// How long a workspace upload may take before create is rolled back.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How much output one provisioning step may buffer in the daemon client.
const PROVISION_OUTPUT_CAP: usize = 16 * 1024;

/// How much of a failing command's output travels in an error message.
const ERROR_OUTPUT: usize = 4096;

/// Both streams, labelled, so a diagnostic on stdout is not lost behind a
/// warning on stderr.
fn combined_output(stdout: &str, stderr: &str) -> String {
    let mut output = String::new();
    if !stdout.is_empty() {
        output.push_str("stdout:\n");
        output.push_str(stdout);
    }
    if !stderr.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str("stderr:\n");
        output.push_str(stderr);
    }
    output
}

fn bounded_output(text: &str) -> String {
    if text.chars().count() <= ERROR_OUTPUT {
        return text.to_string();
    }
    let mut bounded: String = text.chars().take(ERROR_OUTPUT).collect();
    bounded.push_str("\n[truncated]");
    bounded
}

/// The labels that let enumeration report identity without guessing.
const LABEL_NAME: &str = "litterbox.name";
const LABEL_PROJECT: &str = "litterbox.project";
const LABEL_FORWARDS: &str = "litterbox.forwards";
const LABEL_ENVIRONMENT: &str = "litterbox.environment";

fn labels_for(
    request: &CreateRequest,
    forwards: &[Forward],
    provenance: &Provenance,
) -> Result<HashMap<String, String>, Error> {
    let forwards = serde_json::to_string(forwards)
        .map_err(|error| Error::failed(format!("could not label the forwards: {error}")))?;
    let environment = serde_json::to_string(provenance)
        .map_err(|error| Error::failed(format!("could not label the environment: {error}")))?;

    Ok(HashMap::from([
        (LABEL_NAME.to_string(), request.name.clone()),
        (LABEL_PROJECT.to_string(), request.project_slug.clone()),
        (LABEL_FORWARDS.to_string(), forwards),
        (LABEL_ENVIRONMENT.to_string(), environment),
    ]))
}

fn decode_label<T: serde::de::DeserializeOwned>(label: &str, value: &str) -> Result<T, Error> {
    serde_json::from_str(value)
        .map_err(|error| Error::failed(format!("sandbox label '{label}' is unreadable: {error}")))
}

/// A record rebuilt from the labels the backend wrote at create.
///
/// A container without them predates the labels; enumeration skips it rather
/// than inventing a name or provenance it never knew.
fn record_from_labels(
    container: String,
    labels: &HashMap<String, String>,
) -> Result<Option<SandboxRecord>, Error> {
    let (Some(name), Some(project_slug), Some(forwards), Some(environment)) = (
        labels.get(LABEL_NAME),
        labels.get(LABEL_PROJECT),
        labels.get(LABEL_FORWARDS),
        labels.get(LABEL_ENVIRONMENT),
    ) else {
        return Ok(None);
    };

    let forwards: Vec<Forward> = decode_label(LABEL_FORWARDS, forwards)?;
    let environment: Provenance = decode_label(LABEL_ENVIRONMENT, environment)?;

    let data = DockerData {
        container,
        forwards,
        environment,
    };
    Ok(Some(SandboxRecord {
        name: name.clone(),
        backend: BACKEND.to_string(),
        project_slug: project_slug.clone(),
        data: data.into_value()?,
    }))
}

/// How many times a conflicting create is retried with fresh ports.
const PORT_RETRIES: usize = 2;

fn is_name_conflict(error: &ComputeError) -> bool {
    matches!(
        error,
        ComputeError::ContainerProvision { source } if source.kind().is_conflict()
    ) && !is_port_conflict(error)
}

/// A daemon bind failure for the host ports a create asked for.
fn is_port_conflict(error: &ComputeError) -> bool {
    matches!(error, ComputeError::ContainerProvision { .. })
        && error.to_string().contains("port is already allocated")
}

/// The next attempt number when a taken host port is worth retrying.
fn retry_after_port_conflict(error: &ComputeError, attempt: usize) -> Option<usize> {
    (is_port_conflict(error) && attempt < PORT_RETRIES).then_some(attempt + 1)
}

fn is_missing(error: &ComputeError) -> bool {
    match error {
        ComputeError::ContainerPause { source }
        | ComputeError::ContainerResume { source }
        | ComputeError::ContainerExec { source }
        | ComputeError::ContainerInspect { source }
        | ComputeError::ContainerUpload { source }
        | ComputeError::ContainerDownload { source } => source.kind().is_not_found(),
        _ => false,
    }
}

/// Reject a transfer path that is not the workspace or beneath it.
///
/// Both directions are rooted at the workspace the backend reports, so a
/// caller cannot read or write elsewhere in the sandbox.
fn confined(path: &str, sandbox: &SandboxRecord) -> Result<String, Error> {
    use std::path::Component;

    let inside = path == WORKDIR || path.starts_with(&format!("{WORKDIR}/"));
    let escapes = std::path::Path::new(path)
        .components()
        .any(|component| matches!(component, Component::ParentDir));

    if !inside || escapes {
        return Err(Error::Denied {
            operation: format!("transfer '{}' in sandbox '{}'", path, sandbox.name),
            reason: format!("the path is outside the sandbox workspace at {WORKDIR}"),
        });
    }

    Ok(path.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        BACKEND, Backend, CREATE_BUDGET, DockerBackend, DockerSettings, PORT_RETRIES,
        bounded_output, combined_output, command_failure, confined, is_name_conflict,
        is_port_conflict, labels_for, missing_image_check, record_from_labels,
        retry_after_port_conflict, run_steps, step_failure, timeout_failure, upload_failure,
    };
    use crate::backends::docker::compute;
    use crate::backends::docker::error::{BackendError, ComputeError, FailureKind};
    use crate::sandbox::contract::{
        CreateRequest, Error, Execution, Forward, ProvisionStep, ProvisioningClass, RequestedPort,
        SandboxRecord, Scope, Workspace,
    };
    use crate::sandbox::environment::{Origin, Provenance};
    use crate::sandbox::facets::{Limits, ResourceLimits};
    use crate::sandbox::report::{EgressPosture, State};
    use std::collections::HashMap;
    use std::time::Duration;

    fn request() -> CreateRequest {
        CreateRequest {
            name: "demo".to_string(),
            project_slug: "project-1234".to_string(),
            workspace: Workspace {
                source: std::path::PathBuf::from("/tmp/workspace"),
            },
            provision: Vec::new(),
            forward_bind: "127.0.0.1".to_string(),
            ports: Vec::new(),
        }
    }

    #[test]
    fn enumeration_rebuilds_the_record_from_labels() {
        let request = request();
        let forwards = vec![Forward {
            name: "web".to_string(),
            target: 8080,
            host_port: 49152,
            bind: "127.0.0.1".to_string(),
            env_var: "LITTERBOX_FWD_PORT_WEB".to_string(),
        }];
        let provenance = Provenance {
            origin: Origin::Backend {
                from: "the Docker image".to_string(),
            },
            variables: Vec::new(),
            host_paths: Vec::new(),
            steps: Vec::new(),
        };
        let labels = labels_for(&request, &forwards, &provenance).expect("labels");

        let record = record_from_labels("container-id".to_string(), &labels)
            .expect("readable labels")
            .expect("a labelled container");

        assert_eq!(record.name, "demo");
        assert_eq!(record.project_slug, "project-1234");
        assert_eq!(record.backend, BACKEND);
        let data: super::DockerData = record.data.try_into().expect("payload");
        assert_eq!(data.container, "container-id");
        assert_eq!(data.forwards, forwards);
        assert_eq!(data.environment, provenance);
        assert!(
            data.environment.steps.is_empty(),
            "a label carries the base environment, not completed steps"
        );

        assert!(
            record_from_labels("old".to_string(), &HashMap::new())
                .expect("no labels")
                .is_none(),
            "an unlabelled container is skipped rather than invented"
        );
    }

    fn provision(kind: FailureKind, message: &str) -> ComputeError {
        ComputeError::ContainerProvision {
            source: BackendError::new(kind, message.to_string()),
        }
    }

    #[test]
    fn a_missing_container_is_classified_for_every_operation() {
        let missing = || BackendError::new(FailureKind::NotFound, "no such container");
        let errors = [
            ComputeError::ContainerExec { source: missing() },
            ComputeError::ContainerUpload { source: missing() },
            ComputeError::ContainerDownload { source: missing() },
            ComputeError::ContainerInspect { source: missing() },
            ComputeError::ContainerPause { source: missing() },
            ComputeError::ContainerResume { source: missing() },
        ];

        for error in errors {
            assert!(super::is_missing(&error), "{error:?}");
        }

        let conflict = provision(FailureKind::Conflict, "already in use");
        assert!(!super::is_missing(&conflict));
    }

    #[test]
    fn a_taken_host_port_is_retried_but_a_taken_name_is_not() {
        let port = provision(FailureKind::Conflict, "port is already allocated");
        assert!(is_port_conflict(&port));
        assert!(!is_name_conflict(&port));
        assert_eq!(retry_after_port_conflict(&port, 0), Some(1));
        assert_eq!(retry_after_port_conflict(&port, 1), Some(2));
        assert_eq!(retry_after_port_conflict(&port, PORT_RETRIES), None);

        let name = provision(FailureKind::Conflict, "container name is already in use");
        assert!(!is_port_conflict(&name));
        assert!(is_name_conflict(&name));
        assert_eq!(retry_after_port_conflict(&name, 0), None);
    }

    fn record() -> SandboxRecord {
        SandboxRecord {
            name: "demo".to_string(),
            backend: BACKEND.to_string(),
            project_slug: "project".to_string(),
            data: toml::Value::Table(Default::default()),
        }
    }

    #[test]
    fn transfers_inside_the_workspace_are_allowed() {
        for path in ["/src", "/src/README.md", "/src/deep/path"] {
            assert!(
                confined(path, &record()).is_ok(),
                "{path} should be allowed"
            );
        }
    }

    #[test]
    fn transfers_outside_the_workspace_are_denied() {
        for path in ["/etc/passwd", "/", "/src/../etc/passwd", "/srcx/file"] {
            let error = confined(path, &record()).expect_err("escaping path denied");
            assert!(matches!(
                error,
                crate::sandbox::contract::Error::Denied { .. }
            ));
        }
    }

    fn backend() -> DockerBackend {
        DockerBackend::new(
            compute::DockerCompute::connect().expect("docker available"),
            DockerSettings {
                image: "busybox:latest".to_string(),
                ..DockerSettings::default()
            },
        )
    }

    #[test]
    fn an_unset_image_is_a_structured_readiness_check() {
        let check = missing_image_check("").expect("the image is missing");
        assert_eq!(check.state, State::Degraded);
        assert!(check.reason.unwrap().contains("backends.docker.image"));
        assert!(check.remedy.unwrap().contains("user configuration"));

        assert!(missing_image_check("busybox:latest").is_none());
    }

    #[tokio::test]
    async fn capabilities_name_what_docker_provides() {
        let capabilities = backend().capabilities().await.expect("capabilities");
        let describe = |operation: &str| {
            capabilities
                .iter()
                .find(|capability| capability.operation == operation)
                .unwrap_or_else(|| panic!("{operation} is reported: {capabilities:?}"))
        };

        assert_eq!(
            describe("proxied egress").state,
            State::Unavailable,
            "Docker's default bridge is direct, not proxied"
        );
        assert_eq!(
            capabilities.len(),
            1,
            "the backend adds only its mechanism-specific entry; Selected derives the rest: \
             {capabilities:?}"
        );
    }

    #[tokio::test]
    async fn an_unset_image_is_a_configuration_error_at_create() {
        let backend = DockerBackend::new(
            compute::DockerCompute::connect().expect("docker client"),
            DockerSettings {
                image: String::new(),
                ..DockerSettings::default()
            },
        );

        let error = backend.create(&request()).await.expect_err("no image");
        assert!(matches!(error, Error::Misconfigured { .. }), "{error:?}");
        assert!(
            error.to_string().contains("backends.docker.image"),
            "{error}"
        );
    }

    #[test]
    fn output_is_bounded_at_the_boundary() {
        let exact = "a".repeat(super::ERROR_OUTPUT);
        assert_eq!(bounded_output(&exact), exact);
        let over = "a".repeat(super::ERROR_OUTPUT + 1);
        let bounded = bounded_output(&over);
        assert!(bounded.ends_with("[truncated]"), "{bounded}");
        let kept = bounded.trim_end_matches("\n[truncated]");
        assert_eq!(kept.chars().count(), super::ERROR_OUTPUT);
    }

    fn step(identity: &str) -> ProvisionStep {
        step_with(identity, "true")
    }

    fn step_with(identity: &str, run: &str) -> ProvisionStep {
        ProvisionStep::Command {
            identity: identity.to_string(),
            run: run.to_string(),
        }
    }

    #[test]
    fn the_create_budget_is_fixed() {
        assert_eq!(CREATE_BUDGET, Duration::from_secs(2 * 60 * 60));
    }

    #[test]
    fn a_timeout_names_the_step_and_the_budget_without_a_phantom_step() {
        match timeout_failure("fetch", Duration::from_secs(60)) {
            Error::Provisioning { failure } => {
                assert_eq!(failure.step.as_deref(), Some("fetch"));
                assert_eq!(failure.kind.as_deref(), Some("command"));
                assert!(failure.reason.contains("budget"), "{failure:?}");
            }
            other => panic!("expected Provisioning, got {other:?}"),
        }

        match timeout_failure("", Duration::from_secs(60)) {
            Error::Provisioning { failure } => {
                assert_eq!(failure.step, None);
                assert_eq!(failure.kind, None);
                assert!(!failure.reason.contains("a step"), "{failure:?}");
            }
            other => panic!("expected Provisioning, got {other:?}"),
        }
    }

    #[test]
    fn a_step_deadline_names_the_deadline_and_not_a_missing_binary() {
        let step = step("fetch");
        match step_failure(&step, ComputeError::Timeout { seconds: 30 }) {
            Error::Provisioning { failure } => {
                assert_eq!(failure.class, ProvisioningClass::Failed);
                assert_eq!(failure.step.as_deref(), Some("fetch"));
                assert!(failure.reason.contains("30s deadline"), "{failure:?}");
                assert!(!failure.reason.contains("could not run"), "{failure:?}");
                assert!(failure.exit_code.is_none());
            }
            other => panic!("expected Provisioning, got {other}"),
        }
    }

    #[test]
    fn a_daemon_fault_during_a_step_is_unavailable() {
        let error = step_failure(
            &step("fetch"),
            ComputeError::Connection {
                source: BackendError::other("connection refused"),
            },
        );
        assert!(matches!(error, Error::Unavailable { .. }), "{error:?}");
    }

    #[test]
    fn a_command_failure_carries_the_step_exit_and_bounded_output() {
        let step = step("fetch");
        let output = "x".repeat(super::ERROR_OUTPUT + 1);
        match command_failure(&step, 7, &output) {
            Error::Provisioning { failure } => {
                assert_eq!(failure.step.as_deref(), Some("fetch"));
                assert_eq!(failure.kind.as_deref(), Some("command"));
                assert_eq!(failure.exit_code, Some(7));
                assert!(!failure.remedy.is_empty());
                let excerpt = failure.output.expect("output");
                assert!(excerpt.ends_with("[truncated]"), "{excerpt}");
            }
            other => panic!("expected Provisioning, got {other}"),
        }
    }

    #[test]
    fn an_upload_deadline_is_an_unavailable_host_failure() {
        let error = upload_failure(Duration::from_secs(60));
        match error {
            Error::Unavailable { reason, remedy, .. } => {
                assert!(reason.contains("transfer deadline"), "{reason}");
                assert!(!remedy.is_empty());
            }
            other => panic!("expected Unavailable, got {other}"),
        }
    }

    #[tokio::test]
    async fn the_step_loop_returns_only_completed_steps_in_order() {
        let steps = vec![
            step_with("z-first", "touch first"),
            step_with("a-second", "cat first"),
        ];
        let last = std::sync::Mutex::new(String::new());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = seen.clone();
        let completed = run_steps("demo", &steps, &last, move |identity, run| {
            recorded
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push((identity.to_string(), run.to_string()));
            Box::pin(async {
                Ok(Execution {
                    exit_code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            })
        })
        .await
        .expect("steps complete");
        let identities: Vec<&str> = completed
            .iter()
            .map(|step| step.identity.as_str())
            .collect();
        assert_eq!(identities, vec!["z-first", "a-second"]);
        assert_eq!(
            *seen.lock().unwrap_or_else(|poison| poison.into_inner()),
            vec![
                ("z-first".to_string(), "touch first".to_string()),
                ("a-second".to_string(), "cat first".to_string()),
            ],
            "each step runs with its own command, in list order"
        );
        assert_eq!(
            *last.lock().unwrap_or_else(|poison| poison.into_inner()),
            "a-second"
        );
    }

    #[tokio::test]
    async fn the_step_loop_names_the_failing_step_and_keeps_completed_ones() {
        let steps = vec![step("first"), step("second")];
        let last = std::sync::Mutex::new(String::new());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = seen.clone();
        let error = run_steps("demo", &steps, &last, move |identity, run| {
            recorded
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push((identity.to_string(), run.to_string()));
            let failed = identity == "second";
            Box::pin(async move {
                Ok(Execution {
                    exit_code: if failed { 7 } else { 0 },
                    stdout: String::new(),
                    stderr: if failed {
                        "boom".to_string()
                    } else {
                        String::new()
                    },
                })
            })
        })
        .await
        .expect_err("second fails");
        match error {
            Error::Provisioning { failure } => {
                assert_eq!(failure.step.as_deref(), Some("second"));
                assert_eq!(failure.exit_code, Some(7));
                assert!(
                    failure
                        .output
                        .as_deref()
                        .is_some_and(|o| o.contains("boom"))
                );
            }
            other => panic!("expected Provisioning, got {other}"),
        }
        assert_eq!(
            *seen.lock().unwrap_or_else(|poison| poison.into_inner()),
            vec![
                ("first".to_string(), "true".to_string()),
                ("second".to_string(), "true".to_string()),
            ],
            "the failing step still ran with its own command"
        );
        assert_eq!(
            *last.lock().unwrap_or_else(|poison| poison.into_inner()),
            "second"
        );
    }

    #[test]
    fn a_failing_step_excerpt_keeps_both_streams() {
        let output = combined_output("the diagnostic", "a warning");
        assert!(output.contains("stdout:\nthe diagnostic"), "{output}");
        assert!(output.contains("stderr:\na warning"), "{output}");
    }

    #[test]
    fn a_label_carries_the_base_environment_without_steps() {
        let environment = super::environment(&[]);
        let labels = labels_for(&request(), &[], &environment.provenance).expect("labels");
        let provenance: Provenance =
            serde_json::from_str(&labels[super::LABEL_ENVIRONMENT]).expect("environment label");
        assert!(
            provenance.steps.is_empty(),
            "a container label must not claim a step that has not run"
        );
    }

    #[tokio::test]
    #[ignore = "requires a Docker daemon; run with --ignored and LITTERBOX_DOCKER_TESTS=1"]
    async fn create_inspect_and_delete_are_idempotent() -> Result<(), Box<dyn std::error::Error>> {
        if std::env::var("LITTERBOX_DOCKER_TESTS").is_err() {
            return Ok(());
        }

        let backend = backend();
        let workspace = tempfile::tempdir()?;
        std::fs::write(workspace.path().join("README.md"), "hello")?;

        let request = CreateRequest {
            provision: Vec::new(),
            forward_bind: "127.0.0.1".to_string(),
            name: format!("contract-{}", std::process::id()),
            project_slug: "contract-test".to_string(),
            workspace: Workspace {
                source: workspace.path().to_path_buf(),
            },
            ports: vec![RequestedPort {
                name: "web".to_string(),
                target: 8080,
            }],
        };

        let record = backend.create(&request).await?;
        assert_eq!(record.backend, BACKEND);

        let inspection = backend.inspect(&record).await?;
        assert_eq!(inspection.egress, EgressPosture::Direct);
        assert_eq!(inspection.workspace, "/src");
        assert_eq!(inspection.forwards.len(), 1, "the port is forwarded");
        assert_eq!(inspection.forwards[0].target, 8080);
        assert_eq!(inspection.forwards[0].bind, "127.0.0.1");

        let listed = backend
            .list(&Scope::Repository {
                project_slug: "contract-test".to_string(),
            })
            .await?;
        let listed = listed
            .iter()
            .find(|entry| entry.name == record.name)
            .expect("the created sandbox is enumerable");
        assert_eq!(listed.project_slug, "contract-test");
        assert_eq!(listed.backend, BACKEND);

        backend.delete(&record).await?;
        backend.delete(&record).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires a Docker daemon; run with --ignored and LITTERBOX_DOCKER_TESTS=1"]
    async fn the_docker_backend_conforms() -> Result<(), Box<dyn std::error::Error>> {
        if std::env::var("LITTERBOX_DOCKER_TESTS").is_err() {
            return Ok(());
        }

        let backend = backend();
        crate::sandbox::conformance::exercise(&backend).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires a Docker daemon; run with --ignored and LITTERBOX_DOCKER_TESTS=1"]
    async fn command_steps_run_in_order_in_the_workspace() -> Result<(), Box<dyn std::error::Error>>
    {
        if std::env::var("LITTERBOX_DOCKER_TESTS").is_err() {
            return Ok(());
        }

        let backend = backend();
        let workspace = tempfile::tempdir()?;
        std::fs::write(workspace.path().join("README.md"), "hello")?;
        let request = CreateRequest {
            name: crate::sandbox::conformance::run_name("provision"),
            provision: vec![
                ProvisionStep::Command {
                    identity: "z-write".to_string(),
                    run: "printf setup > setup-marker".to_string(),
                },
                ProvisionStep::Command {
                    identity: "a-env".to_string(),
                    run: "printf '%s' \"$LITTERBOX_FWD_PORT_WEB\" > port-marker".to_string(),
                },
                ProvisionStep::Command {
                    identity: "b-read".to_string(),
                    run: "cat setup-marker > read-marker".to_string(),
                },
            ],
            forward_bind: "127.0.0.1".to_string(),
            project_slug: "contract-test".to_string(),
            workspace: Workspace {
                source: workspace.path().to_path_buf(),
            },
            ports: vec![RequestedPort {
                name: "web".to_string(),
                target: 8080,
            }],
        };

        let record = backend.create(&request).await?;
        let inspection = backend.inspect(&record).await?;
        let identities: Vec<&str> = inspection
            .environment
            .steps
            .iter()
            .map(|step| step.identity.as_str())
            .collect();
        assert_eq!(
            identities,
            vec!["z-write", "a-env", "b-read"],
            "steps run in list order"
        );

        let execution = backend
            .exec(
                &record,
                &[
                    "sh".to_string(),
                    "-c".to_string(),
                    "cat read-marker".to_string(),
                ],
            )
            .await?;
        assert_eq!(execution.exit_code, 0, "{}", execution.stderr);
        assert_eq!(
            execution.stdout, "setup",
            "a later step read, relative to the workspace, the file an earlier step wrote"
        );

        let execution = backend
            .exec(
                &record,
                &[
                    "sh".to_string(),
                    "-c".to_string(),
                    "cat port-marker".to_string(),
                ],
            )
            .await?;
        assert_eq!(execution.exit_code, 0, "{}", execution.stderr);
        assert!(
            execution.stdout.trim().parse::<u16>().is_ok(),
            "the step saw the forwarded-port environment: {:?}",
            execution.stdout
        );

        backend.delete(&record).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires a Docker daemon; run with --ignored and LITTERBOX_DOCKER_TESTS=1"]
    async fn a_failed_command_step_names_the_step_and_rolls_back()
    -> Result<(), Box<dyn std::error::Error>> {
        if std::env::var("LITTERBOX_DOCKER_TESTS").is_err() {
            return Ok(());
        }

        let backend = backend();
        let workspace = tempfile::tempdir()?;
        std::fs::write(workspace.path().join("README.md"), "hello")?;
        let request = CreateRequest {
            name: crate::sandbox::conformance::run_name("provision-fail"),
            provision: vec![ProvisionStep::Command {
                identity: "broken".to_string(),
                run: "echo boom >&2; exit 7".to_string(),
            }],
            forward_bind: "127.0.0.1".to_string(),
            project_slug: "contract-test".to_string(),
            workspace: Workspace {
                source: workspace.path().to_path_buf(),
            },
            ports: Vec::new(),
        };

        let error = backend.create(&request).await.expect_err("create fails");
        match &error {
            Error::Provisioning { failure } => {
                assert_eq!(failure.class, ProvisioningClass::Failed);
                assert_eq!(failure.step.as_deref(), Some("broken"));
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
            other => panic!("expected Provisioning, got {other}"),
        }

        let listed = backend.list(&Scope::Machine).await?;
        assert!(
            !listed.iter().any(|record| record.name == request.name),
            "a failed create leaves no container"
        );
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires a Docker daemon; run with --ignored and LITTERBOX_DOCKER_TESTS=1"]
    async fn the_docker_backend_applies_new_limits() -> Result<(), Box<dyn std::error::Error>> {
        if std::env::var("LITTERBOX_DOCKER_TESTS").is_err() {
            return Ok(());
        }

        let backend = backend();
        let workspace = tempfile::tempdir()?;
        std::fs::write(workspace.path().join("README.md"), "hello")?;
        let request = CreateRequest {
            name: crate::sandbox::conformance::run_name("limits"),
            provision: Vec::new(),
            forward_bind: "127.0.0.1".to_string(),
            project_slug: "contract-test".to_string(),
            workspace: Workspace {
                source: workspace.path().to_path_buf(),
            },
            ports: Vec::new(),
        };
        let record = backend.create(&request).await?;

        backend
            .set_limits(
                &record,
                &ResourceLimits {
                    memory_bytes: Some(256 * 1024 * 1024),
                    tasks: Some(128),
                },
            )
            .await?;

        backend.delete(&record).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires a Docker daemon; run with --ignored and LITTERBOX_DOCKER_TESTS=1"]
    async fn the_docker_backend_lifecycle_is_observable() -> Result<(), Box<dyn std::error::Error>>
    {
        if std::env::var("LITTERBOX_DOCKER_TESTS").is_err() {
            return Ok(());
        }

        let backend = backend();
        crate::sandbox::conformance::exercise_lifecycle(&backend).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires a Docker daemon; run with --ignored and LITTERBOX_DOCKER_TESTS=1"]
    async fn the_docker_backend_rolls_back_a_failed_create()
    -> Result<(), Box<dyn std::error::Error>> {
        if std::env::var("LITTERBOX_DOCKER_TESTS").is_err() {
            return Ok(());
        }

        let backend = backend();
        let name = crate::sandbox::conformance::run_name("docker-failure");
        crate::sandbox::conformance::exercise_failed_create_with(
            &backend,
            &name,
            vec![ProvisionStep::Command {
                identity: "fail".to_string(),
                run: "exit 7".to_string(),
            }],
        )
        .await?;
        Ok(())
    }
}
