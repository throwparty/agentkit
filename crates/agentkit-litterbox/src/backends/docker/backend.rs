//! Docker as a sandboxing mechanism.
//!
//! The backend knows about containers, images and bindings. It knows nothing
//! about branches or the project a sandbox belongs to beyond the name it was
//! handed: it is given a prepared tree and asked to confine commands against it.

use std::path::Path;

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

use super::compute;
use super::compute::{ContainerSpec, SandboxState};
use super::naming::container_name_for_slug;
use crate::sandbox::contract::{
    CreateRequest, Error, Execution, Forward, Inspection, Sandbox, SandboxRecord, SandboxStatus,
    Scope,
};
use crate::sandbox::environment::{Change, Origin, Provenance, Variable};
use crate::sandbox::facets::{Forwarding, Lifecycle};
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
    /// The host address forwarded ports bind to.
    pub bind: String,
}

impl Default for DockerSettings {
    fn default() -> Self {
        Self {
            image: String::new(),
            bind: "127.0.0.1".to_string(),
        }
    }
}

/// Addressing data the Docker backend keeps in a sandbox record.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct DockerData {
    container: String,
    forwards: Vec<Forward>,
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

    /// The settings this backend was built with.
    pub fn settings(&self) -> &DockerSettings {
        &self.settings
    }

    /// The client, for callers that need the API directly.
    pub fn compute(&self) -> &compute::DockerCompute {
        &self.compute
    }

    fn container_name(&self, request: &CreateRequest) -> String {
        container_name_for_slug(&request.project_slug, &request.name)
    }

    async fn rollback(&self, container: &str) {
        let _ = self.compute.delete_container(container).await;
    }
}

impl Sandbox for DockerBackend {
    fn create<'a>(
        &'a self,
        request: &'a CreateRequest,
    ) -> BoxFuture<'a, Result<SandboxRecord, Error>> {
        Box::pin(async move {
            self.compute.ensure_image(&self.settings.image).await?;

            let (env, port_bindings, forwards) =
                super::forwarding::forwarded_ports(&request.ports, &self.settings.bind).await?;

            let container = self.container_name(request);
            let spec = ContainerSpec {
                name: container.clone(),
                image: self.settings.image.clone(),
                command: vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "tail -f /dev/null".to_string(),
                ],
                working_dir: Some(WORKDIR.to_string()),
                env,
                port_bindings,
            };

            let id = match self.compute.create_container(&spec).await {
                Ok(id) => id,
                Err(error) if is_name_conflict(&error) => {
                    return Err(Error::Conflict {
                        name: request.name.clone(),
                    });
                }
                Err(error) => return Err(error.into()),
            };

            if let Err(error) = self
                .compute
                .upload_path(&id, &request.workspace.source, &request.workspace.mount)
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
                    return Err(Error::failed(format!(
                        "setup command failed with exit code {}: {stderr}",
                        result.exit_code
                    )));
                }
            }

            let data = DockerData {
                container: id,
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
            let (prefix, project) = match scope {
                Scope::Repository { project_slug } => {
                    (format!("litterbox-{project_slug}-"), project_slug.clone())
                }
                Scope::Machine => ("litterbox-".to_string(), String::new()),
            };

            let summaries = self.compute.list(&prefix).await?;
            let mut records = Vec::new();
            for summary in summaries {
                let Some(name) = summary
                    .names
                    .iter()
                    .find_map(|name| name.strip_prefix(&prefix).map(str::to_string))
                else {
                    continue;
                };
                let data = DockerData {
                    container: summary.id,
                    forwards: Vec::new(),
                };
                records.push(SandboxRecord {
                    name,
                    backend: BACKEND.to_string(),
                    project_slug: project.clone(),
                    data: data.into_value()?,
                });
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
            let result = self
                .compute
                .exec(&data.container, command, Some(WORKDIR))
                .await?;
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
            let data = DockerData::from_record(sandbox)?;
            self.compute
                .upload_path(&data.container, source, destination)
                .await?;
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
            let data = DockerData::from_record(sandbox)?;
            self.compute
                .download_path(&data.container, source, destination)
                .await?;
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
                Ok(SandboxState::Stopped | SandboxState::Missing) => SandboxStatus::Missing,
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
                environment: forwarded_provenance(&data.forwards),
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

            let image = match self.compute.image_present(&self.settings.image).await {
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
                        "pull the image ahead of time to avoid the first-create delay".to_string(),
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
            };

            Ok(Readiness {
                backend: BACKEND.to_string(),
                checks: vec![daemon, image],
            })
        })
    }

    fn capabilities<'a>(&'a self) -> BoxFuture<'a, Result<Vec<Capability>, Error>> {
        Box::pin(async move {
            Ok(vec![
                Capability {
                    operation: "pause and resume".to_string(),
                    state: State::Available,
                    reason: None,
                    remedy: None,
                },
                Capability {
                    operation: "resource limits".to_string(),
                    state: State::Unavailable,
                    reason: Some("the backend does not set limits on the container".to_string()),
                    remedy: Some("bound the workload from inside the container".to_string()),
                },
                Capability {
                    operation: "port forwarding".to_string(),
                    state: State::Available,
                    reason: None,
                    remedy: None,
                },
                Capability {
                    operation: "proxied egress".to_string(),
                    state: State::Unavailable,
                    reason: Some(
                        "the sandbox shares the host network and reports direct egress".to_string(),
                    ),
                    remedy: Some(
                        "run a proxy inside the container or use a network-isolating backend"
                            .to_string(),
                    ),
                },
            ])
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

fn forwarded_provenance(forwards: &[Forward]) -> Provenance {
    Provenance {
        origin: Origin::Backend {
            from: "the Docker image".to_string(),
        },
        variables: forwards
            .iter()
            .map(|forward| Variable {
                name: forward.env_var.clone(),
                change: Change::Set,
            })
            .collect(),
        host_paths: Vec::new(),
    }
}

fn is_name_conflict(error: &crate::domain::SandboxError) -> bool {
    use crate::domain::{ComputeError, SandboxError};
    matches!(
        error,
        SandboxError::Compute(ComputeError::ContainerProvision { source })
            if source.kind().is_conflict()
    )
}

fn is_missing(error: &crate::domain::SandboxError) -> bool {
    use crate::domain::{ComputeError, SandboxError};
    matches!(
        error,
        SandboxError::Compute(
            ComputeError::ContainerPause { source } | ComputeError::ContainerResume { source }
        ) if source.kind().is_not_found()
    )
}

#[cfg(test)]
mod tests {
    use super::{BACKEND, DockerBackend, DockerSettings, Sandbox};
    use crate::backends::docker::compute;
    use crate::sandbox::contract::{CreateRequest, Workspace};
    use crate::sandbox::report::EgressPosture;

    fn backend() -> DockerBackend {
        DockerBackend::new(
            compute::DockerCompute::connect().expect("docker available"),
            DockerSettings {
                image: "busybox:latest".to_string(),
                bind: "127.0.0.1".to_string(),
            },
        )
    }

    #[tokio::test]
    async fn create_inspect_and_delete_are_idempotent() -> Result<(), Box<dyn std::error::Error>> {
        if std::env::var("LITTERBOX_DOCKER_TESTS").is_err() {
            return Ok(());
        }

        let backend = backend();
        let workspace = tempfile::tempdir()?;
        std::fs::write(workspace.path().join("README.md"), "hello")?;

        let request = CreateRequest {
            name: format!("contract-{}", std::process::id()),
            project_slug: "contract-test".to_string(),
            workspace: Workspace {
                source: workspace.path().to_path_buf(),
                mount: "/src".to_string(),
            },
            setup_command: None,
            ports: Vec::new(),
        };

        let record = backend.create(&request).await?;
        assert_eq!(record.backend, BACKEND);

        let inspection = backend.inspect(&record).await?;
        assert_eq!(inspection.egress, EgressPosture::Direct);
        assert_eq!(inspection.workspace, "/src");
        assert!(inspection.forwards.is_empty());

        backend.delete(&record).await?;
        backend.delete(&record).await?;
        Ok(())
    }

    #[tokio::test]
    async fn the_docker_backend_conforms() -> Result<(), Box<dyn std::error::Error>> {
        if std::env::var("LITTERBOX_DOCKER_TESTS").is_err() {
            return Ok(());
        }

        let backend = backend();
        crate::sandbox::conformance::exercise(&backend).await?;
        Ok(())
    }
}
