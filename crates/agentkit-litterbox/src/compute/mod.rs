use std::collections::HashMap;
use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::process::Command;

use bollard::body_full;
use bollard::container::LogOutput;
use bollard::errors::Error as BollardError;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::models::{ContainerCreateBody, HostConfig, PortBinding};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptions, DownloadFromContainerOptionsBuilder,
    ListContainersOptionsBuilder, RemoveContainerOptions, UploadToContainerOptionsBuilder,
};
use bollard::{API_DEFAULT_VERSION, Docker};
use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use tar::{Archive, Builder};

use crate::domain::{BackendError, ComputeError, ExecutionResult, FailureKind, SandboxError};

pub trait Compute {
    fn ensure_image<'a>(&'a self, image: &'a str) -> BoxFuture<'a, Result<(), SandboxError>>;
    fn create_container<'a>(
        &'a self,
        spec: &'a ContainerSpec,
    ) -> BoxFuture<'a, Result<String, SandboxError>>;
    fn inspect_container<'a>(
        &'a self,
        container_id: &'a str,
    ) -> BoxFuture<'a, Result<ContainerInspection, SandboxError>>;
    fn state<'a>(
        &'a self,
        container_id: &'a str,
    ) -> BoxFuture<'a, Result<SandboxState, SandboxError>>;
    /// Sandboxes whose backend name starts with `name_prefix`.
    fn list<'a>(
        &'a self,
        name_prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<SandboxSummary>, SandboxError>>;
    fn pause_container<'a>(
        &'a self,
        container_id: &'a str,
    ) -> BoxFuture<'a, Result<(), SandboxError>>;
    fn resume_container<'a>(
        &'a self,
        container_id: &'a str,
    ) -> BoxFuture<'a, Result<(), SandboxError>>;
    fn delete_container<'a>(
        &'a self,
        container_id: &'a str,
    ) -> BoxFuture<'a, Result<(), SandboxError>>;
    fn exec<'a>(
        &'a self,
        container_id: &'a str,
        command: &'a [String],
        working_dir: Option<&'a str>,
    ) -> BoxFuture<'a, Result<ExecutionResult, SandboxError>>;
    /// Copy a host path into the container at `dest_path`.
    fn upload_path<'a>(
        &'a self,
        container_id: &'a str,
        src_path: &'a Path,
        dest_path: &'a str,
    ) -> BoxFuture<'a, Result<(), SandboxError>>;
    /// Copy a container path into the host `dest_path` directory.
    fn download_path<'a>(
        &'a self,
        container_id: &'a str,
        src_path: &'a str,
        dest_path: &'a Path,
    ) -> BoxFuture<'a, Result<(), SandboxError>>;
}

#[derive(Clone, Debug)]
pub struct ContainerSpec {
    pub name: String,
    pub image: String,
    pub command: Vec<String>,
    pub working_dir: Option<String>,
    pub env: Vec<String>,
    pub port_bindings: HashMap<String, Vec<PortBindingSpec>>,
}

#[derive(Clone, Debug)]
pub struct PortBindingSpec {
    pub host_ip: Option<String>,
    pub host_port: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxState {
    Running,
    Paused,
    Stopped,
    Missing,
}

impl SandboxState {
    pub fn is_active(self) -> bool {
        self == Self::Running
    }
}

#[derive(Clone, Debug)]
pub struct SandboxSummary {
    pub id: String,
    pub names: Vec<String>,
    pub state: SandboxState,
}

pub struct ContainerInspection {
    pub env: Vec<String>,
    pub port_bindings: HashMap<String, Vec<PortBindingSpec>>,
}

pub struct DockerCompute {
    client: Docker,
}

impl DockerCompute {
    pub fn new(client: Docker) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &Docker {
        &self.client
    }

    pub fn connect() -> Result<Self, SandboxError> {
        let client = connect_docker_client()?;
        Ok(Self { client })
    }

    fn connect_with_defaults() -> Result<Docker, SandboxError> {
        Docker::connect_with_local_defaults()
            .map_err(|source| SandboxError::Compute(ComputeError::Connection { source: backend_error(source) }))
    }

    pub async fn ensure_image(&self, image: &str) -> Result<(), SandboxError> {
        match self.client.inspect_image(image).await {
            Ok(_) => Ok(()),
            Err(error) if is_not_found(&error) => self.pull_image(image).await,
            Err(error) => Err(SandboxError::Compute(ComputeError::ImageInspect {
                source: backend_error(error),
            })),
        }
    }

    async fn pull_image(&self, image: &str) -> Result<(), SandboxError> {
        let options = Some(CreateImageOptions {
            from_image: Some(image.to_string()),
            ..Default::default()
        });
        let mut stream = self.client.create_image(options, None, None);

        while let Some(item) = stream.next().await {
            item.map_err(|source| SandboxError::Compute(ComputeError::ImagePull { source: backend_error(source) }))?;
        }

        Ok(())
    }

    pub async fn create_container(&self, spec: &ContainerSpec) -> Result<String, SandboxError> {
        let options = Some(
            CreateContainerOptionsBuilder::default()
                .name(&spec.name)
                .build(),
        );
        let env = if spec.env.is_empty() {
            None
        } else {
            Some(spec.env.clone())
        };
        let port_bindings = if spec.port_bindings.is_empty() {
            None
        } else {
            Some(
                spec.port_bindings
                    .iter()
                    .map(|(key, bindings)| {
                        let bindings = bindings
                            .iter()
                            .map(|binding| PortBinding {
                                host_ip: binding.host_ip.clone(),
                                host_port: binding.host_port.clone(),
                            })
                            .collect();
                        (key.clone(), Some(bindings))
                    })
                    .collect(),
            )
        };
        let config = ContainerCreateBody {
            image: Some(spec.image.clone()),
            cmd: if spec.command.is_empty() {
                None
            } else {
                Some(spec.command.clone())
            },
            working_dir: spec.working_dir.clone(),
            env,
            host_config: Some(HostConfig {
                port_bindings,
                ..Default::default()
            }),
            ..Default::default()
        };

        let response = self
            .client
            .create_container(options, config)
            .await
            .map_err(|source| SandboxError::Compute(ComputeError::ContainerProvision { source: backend_error(source) }))?;

        self.client
            .start_container(&response.id, None)
            .await
            .map_err(|source| SandboxError::Compute(ComputeError::ContainerProvision { source: backend_error(source) }))?;

        Ok(response.id)
    }

    pub async fn inspect_container(
        &self,
        container_id: &str,
    ) -> Result<ContainerInspection, SandboxError> {
        let inspect = self
            .client
            .inspect_container(container_id, None)
            .await
            .map_err(|source| SandboxError::Compute(ComputeError::ContainerInspect { source: backend_error(source) }))?;
        let env = inspect
            .config
            .and_then(|config| config.env)
            .unwrap_or_default();
        let port_bindings = inspect
            .host_config
            .and_then(|config| config.port_bindings)
            .unwrap_or_default()
            .into_iter()
            .map(|(key, bindings)| {
                let bindings = bindings
                    .unwrap_or_default()
                    .into_iter()
                    .map(|binding| PortBindingSpec {
                        host_ip: binding.host_ip,
                        host_port: binding.host_port,
                    })
                    .collect();
                (key, bindings)
            })
            .collect();

        Ok(ContainerInspection { env, port_bindings })
    }

    pub async fn state(&self, container_id: &str) -> Result<SandboxState, SandboxError> {
        match self.client.inspect_container(container_id, None).await {
            Ok(info) => {
                let state = info.state.as_ref();
                if state.and_then(|state| state.paused).unwrap_or(false) {
                    Ok(SandboxState::Paused)
                } else if state.and_then(|state| state.running).unwrap_or(false) {
                    Ok(SandboxState::Running)
                } else {
                    Ok(SandboxState::Stopped)
                }
            }
            Err(error) if is_not_found(&error) => Ok(SandboxState::Missing),
            Err(source) => Err(SandboxError::Compute(ComputeError::ContainerInspect {
                source: backend_error(source),
            })),
        }
    }

    pub async fn list(&self, name_prefix: &str) -> Result<Vec<SandboxSummary>, SandboxError> {
        let options = Some(ListContainersOptionsBuilder::default().all(true).build());
        let containers =
            self.client
                .list_containers(options)
                .await
                .map_err(|source| SandboxError::Compute(ComputeError::ContainerList {
                    source: backend_error(source),
                }))?;

        Ok(containers
            .into_iter()
            .filter(|container| {
                container
                    .names
                    .as_ref()
                    .map(|names| {
                        names
                            .iter()
                            .any(|name| strip_leading_slash(name).starts_with(name_prefix))
                    })
                    .unwrap_or(false)
            })
            .filter_map(|container| {
                let id = container.id?;
                let names = container
                    .names
                    .unwrap_or_default()
                    .iter()
                    .map(|name| strip_leading_slash(name).to_string())
                    .collect();
                let state = match container.state {
                    Some(bollard::models::ContainerSummaryStateEnum::RUNNING) => {
                        SandboxState::Running
                    }
                    Some(bollard::models::ContainerSummaryStateEnum::PAUSED) => {
                        SandboxState::Paused
                    }
                    _ => SandboxState::Stopped,
                };
                Some(SandboxSummary { id, names, state })
            })
            .collect())
    }

    pub async fn pause_container(&self, container_id: &str) -> Result<(), SandboxError> {
        match self.client.pause_container(container_id).await {
            Ok(()) => Ok(()),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 409, ..
            }) => Ok(()),
            Err(source) => Err(SandboxError::Compute(ComputeError::ContainerPause { source: backend_error(source) })),
        }
    }

    pub async fn resume_container(&self, container_id: &str) -> Result<(), SandboxError> {
        match self.client.unpause_container(container_id).await {
            Ok(()) => Ok(()),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 409, ..
            }) => Ok(()),
            Err(source) => Err(SandboxError::Compute(ComputeError::ContainerResume { source: backend_error(source) })),
        }
    }

    pub async fn delete_container(&self, container_id: &str) -> Result<(), SandboxError> {
        match self
            .client
            .remove_container(
                container_id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(()),
            Err(source) => Err(SandboxError::Compute(ComputeError::ContainerDelete { source: backend_error(source) })),
        }
    }

    pub async fn exec(
        &self,
        container_id: &str,
        command: &[String],
        working_dir: Option<&str>,
    ) -> Result<ExecutionResult, SandboxError> {
        let command_args: Vec<&str> = command.iter().map(String::as_str).collect();
        let exec_options = CreateExecOptions {
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            cmd: Some(command_args),
            working_dir,
            ..Default::default()
        };

        let exec = self
            .client
            .create_exec(container_id, exec_options)
            .await
            .map_err(|source| SandboxError::Compute(ComputeError::ContainerExec { source: backend_error(source) }))?;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let results = self
            .client
            .start_exec(&exec.id, None::<StartExecOptions>)
            .await
            .map_err(|source| SandboxError::Compute(ComputeError::ContainerExec { source: backend_error(source) }))?;

        if let StartExecResults::Attached { mut output, .. } = results {
            while let Some(item) = output.next().await {
                match item.map_err(|source| {
                    SandboxError::Compute(ComputeError::ContainerExec { source: backend_error(source) })
                })? {
                    LogOutput::StdOut { message } | LogOutput::Console { message } => {
                        stdout.extend_from_slice(&message)
                    }
                    LogOutput::StdErr { message } => stderr.extend_from_slice(&message),
                    LogOutput::StdIn { .. } => {}
                }
            }
        }

        let inspect = self
            .client
            .inspect_exec(&exec.id)
            .await
            .map_err(|source| SandboxError::Compute(ComputeError::ContainerExec { source: backend_error(source) }))?;
        let exit_code = inspect
            .exit_code
            .unwrap_or(1)
            .try_into()
            .unwrap_or(i32::MAX);

        Ok(ExecutionResult {
            exit_code,
            stdout: String::from_utf8_lossy(&stdout).to_string(),
            stderr: String::from_utf8_lossy(&stderr).to_string(),
        })
    }

    pub async fn upload_path(
        &self,
        container_id: &str,
        src_path: &Path,
        dest_path: &str,
    ) -> Result<(), SandboxError> {
        let tar = build_tar(src_path)?;
        self.upload_tar(container_id, dest_path, &tar).await
    }

    pub async fn download_path(
        &self,
        container_id: &str,
        src_path: &str,
        dest_path: &Path,
    ) -> Result<(), SandboxError> {
        let tar = self.download_tar(container_id, src_path).await?;
        extract_tar(dest_path, &tar)?;
        Ok(())
    }

    async fn upload_tar(
        &self,
        container_id: &str,
        dest_path: &str,
        tar: &[u8],
    ) -> Result<(), SandboxError> {
        let options = Some(
            UploadToContainerOptionsBuilder::default()
                .path(dest_path)
                .build(),
        );
        let body = body_full(Bytes::from(tar.to_vec()));
        self.client
            .upload_to_container(container_id, options, body)
            .await
            .map_err(|source| SandboxError::Compute(ComputeError::ContainerUpload { source: backend_error(source) }))
    }

    async fn download_tar(
        &self,
        container_id: &str,
        src_path: &str,
    ) -> Result<Vec<u8>, SandboxError> {
        let options = Some(
            DownloadFromContainerOptionsBuilder::default()
                .path(src_path)
                .build(),
        );
        let mut stream = self.client.download_from_container(container_id, options);
        let mut buffer = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|source| {
                SandboxError::Compute(ComputeError::ContainerDownload { source: backend_error(source) })
            })?;
            buffer.extend_from_slice(&chunk);
        }
        Ok(buffer)
    }
}

fn connect_docker_client() -> Result<Docker, SandboxError> {
    match select_endpoint_source(docker_context_from_env(), docker_host_from_env()) {
        EndpointSource::ContextEnv { name } => {
            let host = docker_host_for_context(Some(&name)).ok_or_else(|| {
                SandboxError::Config(format!(
                    "DOCKER_CONTEXT is set to '{name}' but the docker CLI could not resolve it"
                ))
            })?;
            connect_with_host(&host)
        }
        EndpointSource::HostEnv { host } => connect_with_host(&host),
        EndpointSource::CliContext => match docker_host_for_context(None) {
            Some(host) => connect_with_host(&host),
            None => DockerCompute::connect_with_defaults(),
        },
    }
}

/// Which endpoint to use, resolved in the same order as the Docker CLI:
/// `DOCKER_CONTEXT` overrides `DOCKER_HOST`, which overrides the active context.
enum EndpointSource {
    ContextEnv { name: String },
    HostEnv { host: String },
    CliContext,
}

fn select_endpoint_source(context: Option<String>, host: Option<String>) -> EndpointSource {
    match (context, host) {
        (Some(name), _) => EndpointSource::ContextEnv { name },
        (None, Some(host)) => EndpointSource::HostEnv { host },
        (None, None) => EndpointSource::CliContext,
    }
}

fn docker_context_from_env() -> Option<String> {
    non_empty_env("DOCKER_CONTEXT")
}

fn docker_host_from_env() -> Option<String> {
    non_empty_env("DOCKER_HOST")
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Asks the docker CLI for a context endpoint. `None` means the active context,
/// which is already shaped by `DOCKER_CONTEXT` and `docker context use`.
fn docker_host_for_context(context: Option<&str>) -> Option<String> {
    let mut command = Command::new("docker");
    command.args(["context", "inspect"]);
    if let Some(context) = context {
        command.arg(context);
    }
    command.args(["-f", "{{.Endpoints.docker.Host}}"]);

    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let host = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if host.is_empty() { None } else { Some(host) }
}

fn connect_with_host(host: &str) -> Result<Docker, SandboxError> {
    let (scheme, rest) = match host.split_once("://") {
        Some((scheme, rest)) => (scheme, rest),
        None => ("unix", host),
    };

    match scheme {
        "unix" => {
            #[cfg(unix)]
            {
                Docker::connect_with_socket(rest, 120, API_DEFAULT_VERSION)
                    .map_err(|source| SandboxError::Compute(ComputeError::Connection { source: backend_error(source) }))
            }
            #[cfg(not(unix))]
            {
                Err(SandboxError::Compute(ComputeError::Connection {
                    source: BackendError::other(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "unix backend host is not supported on this platform",
                    )),
                }))
            }
        }
        "tcp" => {
            let endpoint = format!("http://{}", rest);
            Docker::connect_with_http(&endpoint, 120, API_DEFAULT_VERSION)
                .map_err(|source| SandboxError::Compute(ComputeError::Connection { source: backend_error(source) }))
        }
        _ => DockerCompute::connect_with_defaults(),
    }
}

impl Compute for DockerCompute {
    fn ensure_image<'a>(&'a self, image: &'a str) -> BoxFuture<'a, Result<(), SandboxError>> {
        Box::pin(async move { DockerCompute::ensure_image(self, image).await })
    }

    fn create_container<'a>(
        &'a self,
        spec: &'a ContainerSpec,
    ) -> BoxFuture<'a, Result<String, SandboxError>> {
        Box::pin(async move { DockerCompute::create_container(self, spec).await })
    }

    fn inspect_container<'a>(
        &'a self,
        container_id: &'a str,
    ) -> BoxFuture<'a, Result<ContainerInspection, SandboxError>> {
        Box::pin(async move { DockerCompute::inspect_container(self, container_id).await })
    }

    fn pause_container<'a>(
        &'a self,
        container_id: &'a str,
    ) -> BoxFuture<'a, Result<(), SandboxError>> {
        Box::pin(async move { DockerCompute::pause_container(self, container_id).await })
    }

    fn resume_container<'a>(
        &'a self,
        container_id: &'a str,
    ) -> BoxFuture<'a, Result<(), SandboxError>> {
        Box::pin(async move { DockerCompute::resume_container(self, container_id).await })
    }

    fn delete_container<'a>(
        &'a self,
        container_id: &'a str,
    ) -> BoxFuture<'a, Result<(), SandboxError>> {
        Box::pin(async move { DockerCompute::delete_container(self, container_id).await })
    }

    fn exec<'a>(
        &'a self,
        container_id: &'a str,
        command: &'a [String],
        working_dir: Option<&'a str>,
    ) -> BoxFuture<'a, Result<ExecutionResult, SandboxError>> {
        Box::pin(async move { DockerCompute::exec(self, container_id, command, working_dir).await })
    }

    fn state<'a>(
        &'a self,
        container_id: &'a str,
    ) -> BoxFuture<'a, Result<SandboxState, SandboxError>> {
        Box::pin(async move { DockerCompute::state(self, container_id).await })
    }

    fn list<'a>(
        &'a self,
        name_prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<SandboxSummary>, SandboxError>> {
        Box::pin(async move { DockerCompute::list(self, name_prefix).await })
    }

    fn upload_path<'a>(
        &'a self,
        container_id: &'a str,
        src_path: &'a Path,
        dest_path: &'a str,
    ) -> BoxFuture<'a, Result<(), SandboxError>> {
        Box::pin(async move {
            DockerCompute::upload_path(self, container_id, src_path, dest_path).await
        })
    }

    fn download_path<'a>(
        &'a self,
        container_id: &'a str,
        src_path: &'a str,
        dest_path: &'a Path,
    ) -> BoxFuture<'a, Result<(), SandboxError>> {
        Box::pin(async move {
            DockerCompute::download_path(self, container_id, src_path, dest_path).await
        })
    }
}

fn build_tar(src_path: &Path) -> Result<Vec<u8>, SandboxError> {
    let mut builder = Builder::new(Vec::new());
    if src_path.is_dir() {
        append_dir(&mut builder, src_path, src_path)?;
    } else {
        let name = src_path
            .file_name()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid path"))?;
        builder.append_path_with_name(src_path, name)?;
    }
    builder.finish()?;
    Ok(builder.into_inner()?)
}

fn append_dir(builder: &mut Builder<Vec<u8>>, root: &Path, dir: &Path) -> Result<(), SandboxError> {
    let entries = fs::read_dir(dir)?;
    let mut has_entries = false;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid path"))?;
        has_entries = true;

        if path.is_dir() {
            builder.append_dir(relative, &path)?;
            append_dir(builder, root, &path)?;
        } else if path.is_file() {
            builder.append_path_with_name(&path, relative)?;
        }
    }

    if !has_entries {
        let relative = dir
            .strip_prefix(root)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid path"))?;
        if !relative.as_os_str().is_empty() {
            builder.append_dir(relative, dir)?;
        }
    }

    Ok(())
}

fn extract_tar(dest_path: &Path, tar: &[u8]) -> Result<(), SandboxError> {
    fs::create_dir_all(dest_path)?;
    let mut archive = Archive::new(Cursor::new(tar));

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;

        // Skip .git directory to prevent repository corruption
        if path.starts_with(".git") || path.starts_with("src/.git") {
            continue;
        }

        // Strip leading "src/" or "/src/" from paths to avoid replicating the /src directory
        let stripped_path = path
            .strip_prefix("src/")
            .or_else(|_| path.strip_prefix("/src/"))
            .or_else(|_| path.strip_prefix("src"))
            .unwrap_or(&path);

        // Skip if stripping results in empty path (e.g., if path was exactly "src")
        if stripped_path.as_os_str().is_empty() {
            continue;
        }

        let dest = dest_path.join(stripped_path);

        // Create parent directories if needed
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }

        // Extract the entry to the stripped path
        entry.unpack(&dest)?;
    }

    Ok(())
}

/// Docker summary names carry a leading `/` that container names do not.
fn strip_leading_slash(name: &str) -> &str {
    name.strip_prefix('/').unwrap_or(name)
}

fn backend_error(error: BollardError) -> BackendError {
    let kind = match &error {
        BollardError::DockerResponseServerError {
            status_code: 404, ..
        } => FailureKind::NotFound,
        BollardError::DockerResponseServerError {
            status_code: 409, ..
        } => FailureKind::Conflict,
        _ => FailureKind::Other,
    };
    BackendError::new(kind, error)
}

fn is_not_found(error: &BollardError) -> bool {
    matches!(
        error,
        BollardError::DockerResponseServerError {
            status_code: 404,
            ..
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_names_drop_the_docker_leading_slash() {
        assert_eq!(strip_leading_slash("/litterbox-repo-slug"), "litterbox-repo-slug");
        assert_eq!(strip_leading_slash("litterbox-repo-slug"), "litterbox-repo-slug");
    }

    #[test]
    fn docker_context_env_beats_docker_host_env() {
        let source = select_endpoint_source(
            Some("remote".to_string()),
            Some("unix:///tmp/env.sock".to_string()),
        );
        assert!(matches!(source, EndpointSource::ContextEnv { name } if name == "remote"));
    }

    #[test]
    fn docker_host_env_used_without_context_env() {
        let source = select_endpoint_source(None, Some("unix:///tmp/env.sock".to_string()));
        assert!(matches!(source, EndpointSource::HostEnv { host } if host == "unix:///tmp/env.sock"));
    }

    #[test]
    fn active_context_used_when_neither_env_is_set() {
        assert!(matches!(
            select_endpoint_source(None, None),
            EndpointSource::CliContext
        ));
    }

    #[tokio::test]
    async fn docker_connects_and_ensures_image() -> Result<(), Box<dyn std::error::Error>> {
        // Requires a running Docker daemon; opt in with LITTERBOX_DOCKER_TESTS.
        if std::env::var("LITTERBOX_DOCKER_TESTS").is_err() {
            return Ok(());
        }

        let compute = DockerCompute::connect()?;
        compute.ensure_image("busybox:latest").await?;
        Ok(())
    }
}
