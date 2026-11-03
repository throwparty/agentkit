//! Docker as a sandboxing mechanism.
//!
//! The backend knows about containers, images and bindings. It knows nothing
//! about branches or the project a sandbox belongs to beyond the name it was
//! handed: it is given a prepared tree and asked to confine commands against it.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

use super::compute;
use super::compute::{ContainerSpec, SandboxState};
use super::error::ComputeError;
use super::naming::container_name_for_slug;
use crate::sandbox::contract::{
    Backend, CreateRequest, Error, Execution, Forward, Inspection, SandboxRecord, SandboxStatus,
    Scope,
};
use crate::sandbox::environment::{Change, Environment, Origin, Provenance, Variable};
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

            // A host port can be taken between the probe and the daemon's
            // bind. Both failures surface as a conflict, so a conflict is
            // retried with fresh ports before it is reported as a name clash.
            let container = self.container_name(request);
            let mut attempt = 0;
            let (id, forwards, environment) = loop {
                let (port_bindings, forwards) =
                    super::forwarding::forwarded_ports(&request.ports, &request.forward_bind)
                        .await?;
                let environment = environment(request, &forwards)?;
                let env = environment
                    .variables
                    .iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect();

                let spec = ContainerSpec {
                    name: container.clone(),
                    image: self.settings.image.clone(),
                    command: vec![
                        "sh".to_string(),
                        "-c".to_string(),
                        "tail -f /dev/null".to_string(),
                    ],
                    working_dir: Some(environment.workspace.clone()),
                    env,
                    labels: labels_for(request, &forwards, &environment.provenance)?,
                    port_bindings,
                    memory_bytes: Some((self.settings.memory_mb * 1024 * 1024) as i64),
                    nano_cpus: Some(i64::from(self.settings.cpus) * 1_000_000_000),
                    pids_limit: Some(i64::from(self.settings.pids_limit)),
                    cap_add: self.settings.cap_add.clone(),
                };

                match self.compute.create_container(&spec).await {
                    Ok(id) => break (id, forwards, environment),
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
            };

            if let Err(error) = self
                .compute
                .upload_path(&id, &request.workspace.source, WORKDIR)
                .await
            {
                self.rollback(&id).await;
                return Err(error.into());
            }

            if let Some(command) = &request.setup_command {
                let argv = vec!["sh".to_string(), "-c".to_string(), command.clone()];
                let result = match self.compute.exec(&id, &argv, Some(WORKDIR)).await {
                    Ok(result) => result,
                    Err(error) => {
                        self.rollback(&id).await;
                        return Err(error.into());
                    }
                };
                if result.exit_code != 0 {
                    self.rollback(&id).await;
                    let stderr = if result.stderr.is_empty() {
                        result.stdout
                    } else {
                        result.stderr
                    };
                    tracing::warn!(
                        sandbox = %request.name,
                        output = %stderr,
                        "the setup command failed; the full output is in the log"
                    );
                    return Err(Error::failed(format!(
                        "setup command failed with exit code {}: {}",
                        result.exit_code,
                        bounded_output(&stderr)
                    )));
                }
            }

            let data = DockerData {
                container: id,
                environment: environment.provenance,
                forwards,
            };

            Ok(SandboxRecord {
                name: request.name.clone(),
                backend: BACKEND.to_string(),
                project_slug: request.project_slug.clone(),
                data: data.into_value()?,
            })
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
                if let Scope::Repository { project_slug } = scope
                    && &record.project_slug != project_slug
                {
                    continue;
                }
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
            // The standard facet capabilities are derived by `Selected`.
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

/// Absolute paths a captured value names, unless they are already inside the
/// workspace. They are reported, never granted.
fn host_paths(variables: &[(String, String)]) -> Vec<crate::sandbox::environment::HostPath> {
    let mut paths = Vec::new();
    let workspace_prefix = format!("{WORKDIR}/");
    for (name, value) in variables {
        for part in value.split(':') {
            let inside_workspace = part == WORKDIR || part.starts_with(&workspace_prefix);
            if part.starts_with('/') && !inside_workspace {
                paths.push(crate::sandbox::environment::HostPath {
                    variable: name.clone(),
                    path: std::path::PathBuf::from(part),
                });
            }
        }
    }
    paths
}

/// The backend-owned environment: contents and provenance in one value.
fn environment(request: &CreateRequest, forwards: &[Forward]) -> Result<Environment, Error> {
    let (origin, mut values, mut variables, host_paths) = match &request.environment {
        crate::sandbox::contract::EnvironmentSource::Captured { from, variables } => {
            let mut values = BTreeMap::new();
            for (name, value) in variables {
                if !valid_env_name(name) {
                    return Err(Error::failed(format!(
                        "'{name}' is not a usable environment variable name"
                    )));
                }
                values.insert(name.clone(), value.clone());
            }
            let provenance_variables = variables
                .iter()
                .map(|(name, _)| Variable {
                    name: name.clone(),
                    change: Change::Set,
                })
                .collect();
            (
                Origin::Captured { from: from.clone() },
                values,
                provenance_variables,
                host_paths(variables),
            )
        }
        crate::sandbox::contract::EnvironmentSource::Mechanism => (
            Origin::Backend {
                from: "the Docker image".to_string(),
            },
            BTreeMap::new(),
            Vec::new(),
            Vec::new(),
        ),
    };

    for forward in forwards {
        values.insert(forward.env_var.clone(), forward.host_port.to_string());
        variables.push(Variable {
            name: forward.env_var.clone(),
            change: Change::Set,
        });
    }

    Ok(Environment {
        variables: values,
        provenance: Provenance {
            origin,
            variables,
            host_paths,
        },
        workspace: WORKDIR.to_string(),
    })
}

/// How much of a failing command's output travels in an error message.
const ERROR_OUTPUT: usize = 4096;

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

/// A name the container runtime will accept as a variable.
fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
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
        BACKEND, Backend, DockerBackend, DockerSettings, PORT_RETRIES, confined, environment,
        is_name_conflict, is_port_conflict, labels_for, missing_image_check, record_from_labels,
        retry_after_port_conflict,
    };
    use crate::backends::docker::compute;
    use crate::backends::docker::error::{BackendError, ComputeError, FailureKind};
    use crate::sandbox::contract::{
        CreateRequest, EnvironmentSource, Error, Forward, RequestedPort, SandboxRecord, Scope,
        Workspace,
    };
    use crate::sandbox::environment::{Origin, Provenance};
    use crate::sandbox::facets::{Limits, ResourceLimits};
    use crate::sandbox::report::{EgressPosture, State};
    use std::collections::HashMap;

    fn request() -> CreateRequest {
        CreateRequest {
            name: "demo".to_string(),
            project_slug: "project-1234".to_string(),
            workspace: Workspace {
                source: std::path::PathBuf::from("/tmp/workspace"),
            },
            environment: EnvironmentSource::Mechanism,
            forward_bind: "127.0.0.1".to_string(),
            setup_command: None,
            ports: Vec::new(),
        }
    }

    #[test]
    fn the_environment_carries_contents_and_provenance_without_leaking_values() {
        let mut request = request();
        request.environment = EnvironmentSource::Captured {
            from: "the shell".to_string(),
            variables: vec![("TOKEN".to_string(), "SECRET-VALUE".to_string())],
        };

        let environment = environment(&request, &[]).expect("environment");
        assert_eq!(
            environment.variables.get("TOKEN").map(String::as_str),
            Some("SECRET-VALUE")
        );
        assert_eq!(environment.provenance.variables[0].name, "TOKEN");
        assert_eq!(environment.workspace, "/src");

        let reported = serde_json::to_string(&environment.provenance).expect("serialises");
        assert!(!reported.contains("SECRET-VALUE"), "{reported}");
    }

    #[test]
    fn captured_host_paths_are_reported_not_granted() {
        let mut request = request();
        request.environment = EnvironmentSource::Captured {
            from: "the shell".to_string(),
            variables: vec![
                (
                    "SSH_AUTH_SOCK".to_string(),
                    "/run/user/1000/ssh-agent.sock".to_string(),
                ),
                ("PWD".to_string(), "/src/sub".to_string()),
                (
                    "PATH".to_string(),
                    "/host/bin:/src/bin:/src-tools/bin".to_string(),
                ),
            ],
        };

        let environment = environment(&request, &[]).expect("environment");
        let provenance = environment.provenance;
        let named: Vec<&str> = provenance
            .host_paths
            .iter()
            .map(|path| path.path.to_str().expect("utf-8 path"))
            .collect();
        assert!(
            named.contains(&"/run/user/1000/ssh-agent.sock"),
            "{named:?}"
        );
        assert!(named.contains(&"/host/bin"), "{named:?}");
        assert!(
            named.contains(&"/src-tools/bin"),
            "a path that merely starts with /src is a host path: {named:?}"
        );
        assert!(
            !named
                .iter()
                .any(|path| *path == "/src/sub" || *path == "/src/bin"),
            "{named:?}"
        );
        assert!(
            provenance
                .variables
                .iter()
                .all(|variable| variable.change == crate::sandbox::environment::Change::Set)
        );
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
            "standard facet capabilities are derived by Selected: {capabilities:?}"
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

    #[tokio::test]
    #[ignore = "requires a Docker daemon; run with --ignored and LITTERBOX_DOCKER_TESTS=1"]
    async fn create_inspect_and_delete_are_idempotent() -> Result<(), Box<dyn std::error::Error>> {
        assert!(
            std::env::var("LITTERBOX_DOCKER_TESTS").is_ok(),
            "set LITTERBOX_DOCKER_TESTS=1 and run with --ignored to exercise this test"
        );

        let backend = backend();
        let workspace = tempfile::tempdir()?;
        std::fs::write(workspace.path().join("README.md"), "hello")?;

        let request = CreateRequest {
            environment: crate::sandbox::contract::EnvironmentSource::Mechanism,
            forward_bind: "127.0.0.1".to_string(),
            name: format!("contract-{}", std::process::id()),
            project_slug: "contract-test".to_string(),
            workspace: Workspace {
                source: workspace.path().to_path_buf(),
            },
            setup_command: None,
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
        assert!(
            std::env::var("LITTERBOX_DOCKER_TESTS").is_ok(),
            "set LITTERBOX_DOCKER_TESTS=1 and run with --ignored to exercise this test"
        );

        let backend = backend();
        crate::sandbox::conformance::exercise(&backend).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires a Docker daemon; run with --ignored and LITTERBOX_DOCKER_TESTS=1"]
    async fn a_setup_command_creates_a_file_visible_to_later_commands()
    -> Result<(), Box<dyn std::error::Error>> {
        assert!(
            std::env::var("LITTERBOX_DOCKER_TESTS").is_ok(),
            "set LITTERBOX_DOCKER_TESTS=1 and run with --ignored to exercise this test"
        );

        let backend = backend();
        let workspace = tempfile::tempdir()?;
        std::fs::write(workspace.path().join("README.md"), "hello")?;
        let request = CreateRequest {
            name: crate::sandbox::conformance::run_name("setup"),
            environment: EnvironmentSource::Mechanism,
            forward_bind: "127.0.0.1".to_string(),
            project_slug: "contract-test".to_string(),
            workspace: Workspace {
                source: workspace.path().to_path_buf(),
            },
            setup_command: Some("printf setup > /setup-marker".to_string()),
            ports: Vec::new(),
        };

        let record = backend.create(&request).await?;
        let execution = backend
            .exec(
                &record,
                &[
                    "sh".to_string(),
                    "-c".to_string(),
                    "cat /setup-marker".to_string(),
                ],
            )
            .await?;
        assert_eq!(execution.exit_code, 0, "{}", execution.stderr);
        assert_eq!(
            execution.stdout, "setup",
            "create ran the setup command in the workspace"
        );

        backend.delete(&record).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires a Docker daemon; run with --ignored and LITTERBOX_DOCKER_TESTS=1"]
    async fn the_docker_backend_applies_new_limits() -> Result<(), Box<dyn std::error::Error>> {
        assert!(
            std::env::var("LITTERBOX_DOCKER_TESTS").is_ok(),
            "set LITTERBOX_DOCKER_TESTS=1 and run with --ignored to exercise this test"
        );

        let backend = backend();
        let workspace = tempfile::tempdir()?;
        std::fs::write(workspace.path().join("README.md"), "hello")?;
        let request = CreateRequest {
            name: crate::sandbox::conformance::run_name("limits"),
            environment: EnvironmentSource::Mechanism,
            forward_bind: "127.0.0.1".to_string(),
            project_slug: "contract-test".to_string(),
            workspace: Workspace {
                source: workspace.path().to_path_buf(),
            },
            setup_command: None,
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
        assert!(
            std::env::var("LITTERBOX_DOCKER_TESTS").is_ok(),
            "set LITTERBOX_DOCKER_TESTS=1 and run with --ignored to exercise this test"
        );

        let backend = backend();
        crate::sandbox::conformance::exercise_lifecycle(&backend).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires a Docker daemon; run with --ignored and LITTERBOX_DOCKER_TESTS=1"]
    async fn the_docker_backend_rolls_back_a_failed_create()
    -> Result<(), Box<dyn std::error::Error>> {
        assert!(
            std::env::var("LITTERBOX_DOCKER_TESTS").is_ok(),
            "set LITTERBOX_DOCKER_TESTS=1 and run with --ignored to exercise this test"
        );

        let backend = backend();
        let name = crate::sandbox::conformance::run_name("docker-failure");
        crate::sandbox::conformance::exercise_failed_create_with(&backend, &name, Some("exit 7"))
            .await?;
        Ok(())
    }
}
