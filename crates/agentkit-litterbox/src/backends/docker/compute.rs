//! Docker's API client.
//!
//! Nothing outside `backends::docker` may use this module: it speaks the
//! daemon's vocabulary, and the contract must not.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use bollard::body_try_stream;
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
use tar::{Archive, Builder};

use super::error::{BackendError, ComputeError, FailureKind};
use crate::sandbox::contract::Execution;

/// Everything one container is created with.
#[derive(Clone, Debug)]
pub struct ContainerSpec {
    /// The container's name.
    pub name: String,
    /// The image it runs.
    pub image: String,
    /// Its entry command.
    pub command: Vec<String>,
    /// Where commands start.
    pub working_dir: Option<String>,
    /// `KEY=VALUE` entries.
    pub env: Vec<String>,
    /// Labels enumeration reads back.
    pub labels: HashMap<String, String>,
    /// Ports to publish, keyed by container port.
    pub port_bindings: HashMap<String, Vec<PortBindingSpec>>,
    /// Memory ceiling for the container.
    pub memory_bytes: Option<i64>,
    /// CPU quota in nanoseconds per second.
    pub nano_cpus: Option<i64>,
    /// Maximum tasks or processes.
    pub pids_limit: Option<i64>,
    /// Extra capabilities to grant.
    pub cap_add: Vec<String>,
}

/// One host binding for a container port.
#[derive(Clone, Debug)]
pub struct PortBindingSpec {
    /// Host address the port is published on.
    pub host_ip: Option<String>,
    /// Host port, when one was chosen.
    pub host_port: Option<String>,
}

/// What the daemon says a container is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxState {
    /// Running.
    Running,
    /// Paused.
    Paused,
    /// Created but not running.
    Stopped,
    /// The daemon does not have it.
    Missing,
}

/// One container as enumeration sees it.
#[derive(Clone, Debug)]
pub struct SandboxSummary {
    /// The container id.
    pub id: String,
    /// The labels the backend wrote at create.
    pub labels: HashMap<String, String>,
}

/// A connected Docker client, used only by this backend.
#[derive(Clone)]
pub struct DockerCompute {
    client: Docker,
}

impl DockerCompute {
    pub fn connect() -> Result<Self, ComputeError> {
        let client = connect_docker_client()?;
        Ok(Self { client })
    }

    fn connect_with_defaults() -> Result<Docker, ComputeError> {
        Docker::connect_with_local_defaults().map_err(|source| ComputeError::Connection {
            source: backend_error(source),
        })
    }

    pub async fn ensure_image(&self, image: &str) -> Result<(), ComputeError> {
        match self.client.inspect_image(image).await {
            Ok(_) => Ok(()),
            Err(error) if is_not_found(&error) => self.pull_image(image).await,
            Err(error) => Err(ComputeError::ImageInspect {
                source: backend_error(error),
            }),
        }
    }

    /// Whether the daemon answers.
    pub async fn ping(&self) -> Result<(), ComputeError> {
        self.client
            .ping()
            .await
            .map(|_| ())
            .map_err(|source| ComputeError::Connection {
                source: backend_error(source),
            })
    }

    /// Whether an image is present locally, without pulling it.
    pub async fn image_present(&self, image: &str) -> Result<bool, ComputeError> {
        match self.client.inspect_image(image).await {
            Ok(_) => Ok(true),
            Err(error) if is_not_found(&error) => Ok(false),
            Err(error) => Err(ComputeError::ImageInspect {
                source: backend_error(error),
            }),
        }
    }

    async fn pull_image(&self, image: &str) -> Result<(), ComputeError> {
        let options = Some(CreateImageOptions {
            from_image: Some(image.to_string()),
            ..Default::default()
        });
        let mut stream = self.client.create_image(options, None, None);

        while let Some(item) = stream.next().await {
            item.map_err(|source| ComputeError::ImagePull {
                source: backend_error(source),
            })?;
        }

        Ok(())
    }

    pub async fn create_container(&self, spec: &ContainerSpec) -> Result<String, ComputeError> {
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
            labels: Some(spec.labels.clone()),
            host_config: Some(HostConfig {
                port_bindings,
                pids_limit: spec.pids_limit,
                memory: spec.memory_bytes,
                nano_cpus: spec.nano_cpus,
                cap_add: if spec.cap_add.is_empty() {
                    None
                } else {
                    Some(spec.cap_add.clone())
                },
                cap_drop: Some(vec!["ALL".to_string()]),
                security_opt: Some(vec!["no-new-privileges".to_string()]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let response = self
            .client
            .create_container(options, config)
            .await
            .map_err(|source| ComputeError::ContainerProvision {
                source: backend_error(source),
            })?;

        if let Err(source) = self.client.start_container(&response.id, None).await {
            // The container exists but never ran: remove it so the name stays
            // usable and create leaves nothing behind.
            let _ = self
                .client
                .remove_container(
                    &response.id,
                    Some(RemoveContainerOptions {
                        force: true,
                        ..Default::default()
                    }),
                )
                .await;
            return Err(ComputeError::ContainerProvision {
                source: backend_error(source),
            });
        }

        Ok(response.id)
    }

    pub async fn state(&self, container_id: &str) -> Result<SandboxState, ComputeError> {
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
            Err(source) => Err(ComputeError::ContainerInspect {
                source: backend_error(source),
            }),
        }
    }

    pub async fn list(&self, name_prefix: &str) -> Result<Vec<SandboxSummary>, ComputeError> {
        let options = Some(ListContainersOptionsBuilder::default().all(true).build());
        let containers = self
            .client
            .list_containers(options)
            .await
            .map_err(|source| ComputeError::ContainerList {
                source: backend_error(source),
            })?;

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
                Some(SandboxSummary {
                    id: container.id?,
                    labels: container.labels.unwrap_or_default(),
                })
            })
            .collect())
    }

    /// Replace the resource bounds of a running sandbox.
    pub async fn update_limits(
        &self,
        container_id: &str,
        memory_bytes: Option<u64>,
        tasks: Option<u32>,
        nano_cpus: Option<i64>,
    ) -> Result<(), ComputeError> {
        let body = bollard::models::ContainerUpdateBody {
            memory: memory_bytes.map(|bytes| bytes as i64),
            pids_limit: tasks.map(i64::from),
            nano_cpus,
            ..Default::default()
        };
        self.client
            .update_container(container_id, body)
            .await
            .map(|_| ())
            .map_err(|source| ComputeError::ContainerLimits {
                source: backend_error(source),
            })
    }

    pub async fn pause_container(&self, container_id: &str) -> Result<(), ComputeError> {
        match self.client.pause_container(container_id).await {
            Ok(()) => Ok(()),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 409, ..
            }) => Ok(()),
            Err(source) => Err(ComputeError::ContainerPause {
                source: backend_error(source),
            }),
        }
    }

    pub async fn resume_container(&self, container_id: &str) -> Result<(), ComputeError> {
        match self.client.unpause_container(container_id).await {
            Ok(()) => Ok(()),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 409, ..
            }) => Ok(()),
            Err(source) => Err(ComputeError::ContainerResume {
                source: backend_error(source),
            }),
        }
    }

    pub async fn delete_container(&self, container_id: &str) -> Result<(), ComputeError> {
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
            Err(source) => Err(ComputeError::ContainerDelete {
                source: backend_error(source),
            }),
        }
    }

    pub async fn exec(
        &self,
        container_id: &str,
        command: &[String],
        working_dir: Option<&str>,
    ) -> Result<Execution, ComputeError> {
        self.exec_with(
            container_id,
            command,
            working_dir,
            EXEC_OUTPUT_CAP,
            EXEC_TIMEOUT,
        )
        .await
    }

    /// Run one provisioning step under its own, shorter deadline.
    pub async fn exec_provisioning(
        &self,
        container_id: &str,
        command: &[String],
        working_dir: Option<&str>,
        output_cap: usize,
    ) -> Result<Execution, ComputeError> {
        self.exec_with(
            container_id,
            command,
            working_dir,
            output_cap,
            PROVISION_STEP_TIMEOUT,
        )
        .await
    }

    async fn exec_with(
        &self,
        container_id: &str,
        command: &[String],
        working_dir: Option<&str>,
        output_cap: usize,
        timeout: std::time::Duration,
    ) -> Result<Execution, ComputeError> {
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
            .map_err(|source| ComputeError::ContainerExec {
                source: backend_error(source),
            })?;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let results = self
            .client
            .start_exec(&exec.id, None::<StartExecOptions>)
            .await
            .map_err(|source| ComputeError::ContainerExec {
                source: backend_error(source),
            })?;

        if let StartExecResults::Attached { mut output, .. } = results {
            let collect = async {
                while let Some(item) = output.next().await {
                    match item.map_err(|source| ComputeError::ContainerExec {
                        source: backend_error(source),
                    })? {
                        LogOutput::StdOut { message } | LogOutput::Console { message } => {
                            append_capped(&mut stdout, &message, output_cap)
                        }
                        LogOutput::StdErr { message } => {
                            append_capped(&mut stderr, &message, output_cap)
                        }
                        LogOutput::StdIn { .. } => {}
                    }
                }
                Ok::<(), ComputeError>(())
            };

            // A command that never returns must not wedge the caller, and its
            // output must not exhaust the host.
            if tokio::time::timeout(timeout, collect).await.is_err() {
                self.kill_exec_process(container_id, &exec.id).await;
                return Err(ComputeError::Timeout {
                    seconds: timeout.as_secs(),
                });
            }
        }

        let inspect = self.client.inspect_exec(&exec.id).await.map_err(|source| {
            ComputeError::ContainerExec {
                source: backend_error(source),
            }
        })?;
        let exit_code = inspect
            .exit_code
            .unwrap_or(1)
            .try_into()
            .unwrap_or(i32::MAX);

        Ok(Execution {
            exit_code,
            stdout: String::from_utf8_lossy(&stdout).to_string(),
            stderr: String::from_utf8_lossy(&stderr).to_string(),
        })
    }

    /// Best-effort kill of an exec that outlived its deadline.
    async fn kill_exec_process(&self, container_id: &str, exec_id: &str) {
        let Ok(inspect) = self.client.inspect_exec(exec_id).await else {
            return;
        };
        let Some(pid) = inspect.pid else {
            return;
        };

        let options = CreateExecOptions {
            attach_stdout: Some(false),
            attach_stderr: Some(false),
            cmd: Some(vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("kill -KILL {pid}"),
            ]),
            ..Default::default()
        };
        let Ok(exec) = self.client.create_exec(container_id, options).await else {
            return;
        };
        let started = self
            .client
            .start_exec(
                &exec.id,
                Some(StartExecOptions {
                    detach: true,
                    ..Default::default()
                }),
            )
            .await;
        if let Err(error) = started {
            tracing::warn!(%error, container = %container_id, pid, "could not kill a timed-out command");
        }
    }

    pub async fn upload_path(
        &self,
        container_id: &str,
        src_path: &Path,
        dest_path: &str,
    ) -> Result<(), ComputeError> {
        // Build the archive into a temporary file and stream it to the daemon,
        // so a large workspace is never held in memory.
        let file = write_tar(
            src_path,
            CappedFile::new(tempfile::tempfile()?, EXTRACT_MAX_BYTES),
        )?;
        let mut file = file.into_inner();
        file.seek(SeekFrom::Start(0))?;
        self.upload_stream(container_id, dest_path, file).await
    }

    pub async fn download_path(
        &self,
        container_id: &str,
        src_path: &str,
        dest_path: &Path,
        workspace: &str,
        selector: Option<&dyn crate::sandbox::contract::SnapshotSelector>,
    ) -> Result<(), ComputeError> {
        // The daemon archives a whole path and cannot leave anything out of it,
        // so the archive is filtered as it arrives: a rejected path is never
        // written to the host and never counts toward the transfer limit. What
        // arrives is held in a capped file rather than memory, so a large
        // workspace never has to fit in either.
        let staged = CappedFile::new(tempfile::tempfile()?, EXTRACT_MAX_BYTES);
        let mut kept = SelectingArchive::new(staged, workspace, selector);
        self.download_into(container_id, src_path, &mut kept)
            .await?;
        let mut staged = kept.into_inner().into_inner();
        staged.seek(SeekFrom::Start(0))?;
        extract_tar(dest_path, staged, workspace)?;
        Ok(())
    }

    async fn upload_stream(
        &self,
        container_id: &str,
        dest_path: &str,
        file: fs::File,
    ) -> Result<(), ComputeError> {
        let options = Some(
            UploadToContainerOptionsBuilder::default()
                .path(dest_path)
                .build(),
        );
        let body = body_try_stream(file_chunks(file));
        self.client
            .upload_to_container(container_id, options, body)
            .await
            .map_err(|source| ComputeError::ContainerUpload {
                source: backend_error(source),
            })
    }

    async fn download_into<W: Write>(
        &self,
        container_id: &str,
        src_path: &str,
        destination: &mut SelectingArchive<'_, W>,
    ) -> Result<(), ComputeError> {
        let options = Some(
            DownloadFromContainerOptionsBuilder::default()
                .path(src_path)
                .build(),
        );
        let mut stream = self.client.download_from_container(container_id, options);
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|source| ComputeError::ContainerDownload {
                source: backend_error(source),
            })?;
            destination.select(&chunk)?;
        }
        destination.finish()
    }
}

fn connect_docker_client() -> Result<Docker, ComputeError> {
    // A TLS configuration this client cannot honour must not be silently
    // downgraded to cleartext.
    if std::env::var("DOCKER_TLS_VERIFY").is_ok() {
        return Err(ComputeError::Config(
            "DOCKER_TLS_VERIFY is set but TLS endpoints are not supported; \
             connect over the local socket or unset it"
                .to_string(),
        ));
    }

    if let Some(name) = docker_context_from_env() {
        return Err(ComputeError::Config(format!(
            "DOCKER_CONTEXT '{name}' is not supported; set DOCKER_HOST or unset it"
        )));
    }

    match docker_host_from_env() {
        Some(host) => connect_with_host(&host),
        None => DockerCompute::connect_with_defaults(),
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

fn connect_with_host(host: &str) -> Result<Docker, ComputeError> {
    let (scheme, rest) = match host.split_once("://") {
        Some((scheme, rest)) => (scheme, rest),
        None => ("unix", host),
    };

    match scheme {
        "unix" => {
            #[cfg(unix)]
            {
                Docker::connect_with_socket(rest, 120, API_DEFAULT_VERSION).map_err(|source| {
                    ComputeError::Connection {
                        source: backend_error(source),
                    }
                })
            }
            #[cfg(not(unix))]
            {
                Err(ComputeError::Connection {
                    source: BackendError::other(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "unix backend host is not supported on this platform",
                    )),
                })
            }
        }
        // A cleartext TCP endpoint would send the workspace and every command
        // in the open. TLS is not wired yet, so the only safe answer is no.
        "tcp" => Err(ComputeError::Config(
            "DOCKER_HOST uses tcp://, but remote daemons are not supported; \
             connect over the local unix socket"
                .to_string(),
        )),
        other => Err(ComputeError::Config(format!(
            "DOCKER_HOST scheme '{other}' is not supported; use unix://"
        ))),
    }
}

/// A buffer that refuses to grow past the transfer ceiling.
#[cfg(test)]
struct CappedBuffer {
    bytes: Vec<u8>,
    limit: u64,
}

#[cfg(test)]
impl CappedBuffer {
    fn new(limit: u64) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }

    fn into_vec(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
impl Write for CappedBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len() as u64 + buf.len() as u64 > self.limit {
            return Err(std::io::Error::other(format!(
                "the archive exceeds the transfer limit of {} bytes",
                self.limit
            )));
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A file writer that enforces the transfer ceiling as it writes.
struct CappedFile {
    inner: fs::File,
    written: u64,
    limit: u64,
}

impl CappedFile {
    fn new(inner: fs::File, limit: u64) -> Self {
        Self {
            inner,
            written: 0,
            limit,
        }
    }

    fn into_inner(self) -> fs::File {
        self.inner
    }
}

impl Write for CappedFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.written + buf.len() as u64 > self.limit {
            return Err(std::io::Error::other(format!(
                "the archive exceeds the transfer limit of {} bytes",
                self.limit
            )));
        }
        let written = self.inner.write(buf)?;
        self.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn write_tar<W: Write>(src_path: &Path, writer: W) -> Result<W, ComputeError> {
    let mut builder = Builder::new(writer);
    let metadata = fs::symlink_metadata(src_path)?;
    if metadata.is_dir() {
        append_dir(&mut builder, src_path, src_path)?;
    } else if metadata.is_file() {
        let name = src_path
            .file_name()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid path"))?;
        builder.append_path_with_name(src_path, name)?;
    } else {
        return Err(ComputeError::Config(format!(
            "refusing to transfer a non-regular path: {}",
            src_path.display()
        )));
    }
    builder.finish()?;
    Ok(builder.into_inner()?)
}

#[cfg(test)]
fn build_tar(src_path: &Path) -> Result<Vec<u8>, ComputeError> {
    Ok(write_tar(src_path, CappedBuffer::new(EXTRACT_MAX_BYTES))?.into_vec())
}

/// Read a staged archive back in chunks for the daemon request body.
fn file_chunks(
    file: fs::File,
) -> impl futures_util::Stream<Item = std::io::Result<Bytes>> + Send + 'static {
    futures_util::stream::try_unfold(file, |mut file| async move {
        let mut buffer = vec![0u8; 64 * 1024];
        match file.read(&mut buffer) {
            Ok(0) => Ok(None),
            Ok(read) => {
                buffer.truncate(read);
                Ok(Some((Bytes::from(buffer), file)))
            }
            Err(error) => Err(error),
        }
    })
}

fn append_dir<W: Write>(
    builder: &mut Builder<W>,
    root: &Path,
    dir: &Path,
) -> Result<(), ComputeError> {
    let entries = fs::read_dir(dir)?;
    let mut has_entries = false;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid path"))?;
        has_entries = true;

        // `file_type` does not follow links: a symlink in the workspace is
        // neither transferred nor read through.
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            builder.append_dir(relative, &path)?;
            append_dir(builder, root, &path)?;
        } else if file_type.is_file() {
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

/// The destination-relative path an archive entry lands at, or `None` for
/// entries that are skipped.
fn stripped_entry(path: &Path, workspace: &str) -> Option<std::path::PathBuf> {
    let workspace_git = format!("{workspace}/.git");
    if path.starts_with(".git") || path.starts_with(&workspace_git) {
        return None;
    }

    let workspace_prefix = format!("{workspace}/");
    let stripped = path
        .strip_prefix(&workspace_prefix)
        .or_else(|_| path.strip_prefix(format!("/{workspace_prefix}")))
        .or_else(|_| path.strip_prefix(workspace))
        .unwrap_or(path);

    if stripped.as_os_str().is_empty() {
        None
    } else {
        Some(stripped.to_path_buf())
    }
}

/// The size of a tar header block, and of the padding that rounds an entry up.
const BLOCK: usize = 512;

/// How much of an entry the stream still owes.
#[derive(Clone, Copy)]
enum Body {
    /// A header block is due next.
    Header,
    /// Copy the rest of the entry to the sink.
    Copy(u64),
    /// Drop the rest of the entry.
    Skip(u64),
    /// Hold the rest of a metadata entry until the entry it describes is
    /// decided.
    Hold(u64),
}

/// Forwards only the archive entries a selector keeps.
///
/// The daemon archives a whole path and has no way to leave a path out, so the
/// selection happens on the stream as it arrives: a rejected entry's contents
/// are read and dropped rather than staged. Metadata entries are held back with
/// the entry they describe, so a long name never dangles in front of a file the
/// archive no longer carries.
struct SelectingArchive<'a, W: Write> {
    sink: W,
    workspace: String,
    selector: Option<&'a dyn crate::sandbox::contract::SnapshotSelector>,
    pending: Vec<u8>,
    header: Vec<u8>,
    body: Body,
}

impl<'a, W: Write> SelectingArchive<'a, W> {
    fn new(
        sink: W,
        workspace: &str,
        selector: Option<&'a dyn crate::sandbox::contract::SnapshotSelector>,
    ) -> Self {
        Self {
            sink,
            workspace: workspace.trim_matches('/').to_string(),
            selector,
            pending: Vec::new(),
            header: Vec::with_capacity(BLOCK),
            body: Body::Header,
        }
    }

    fn into_inner(self) -> W {
        self.sink
    }

    fn finish(&mut self) -> Result<(), ComputeError> {
        self.sink.flush()?;
        Ok(())
    }

    /// Take a chunk of the archive, keeping what the selector wants.
    fn select(&mut self, mut chunk: &[u8]) -> Result<(), ComputeError> {
        while !chunk.is_empty() {
            match self.body {
                Body::Copy(remaining) => {
                    let take = remaining.min(chunk.len() as u64) as usize;
                    self.sink.write_all(&chunk[..take])?;
                    chunk = &chunk[take..];
                    self.body = if take as u64 == remaining {
                        Body::Header
                    } else {
                        Body::Copy(remaining - take as u64)
                    };
                }
                Body::Skip(remaining) => {
                    let take = remaining.min(chunk.len() as u64) as usize;
                    chunk = &chunk[take..];
                    self.body = if take as u64 == remaining {
                        Body::Header
                    } else {
                        Body::Skip(remaining - take as u64)
                    };
                }
                Body::Hold(remaining) => {
                    let take = remaining.min(chunk.len() as u64) as usize;
                    self.pending.extend_from_slice(&chunk[..take]);
                    chunk = &chunk[take..];
                    self.body = if take as u64 == remaining {
                        Body::Header
                    } else {
                        Body::Hold(remaining - take as u64)
                    };
                }
                Body::Header => {
                    let want = BLOCK - self.header.len();
                    let take = want.min(chunk.len());
                    self.header.extend_from_slice(&chunk[..take]);
                    chunk = &chunk[take..];
                    if self.header.len() == BLOCK {
                        let block = std::mem::replace(&mut self.header, Vec::with_capacity(BLOCK));
                        self.block(&block)?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Decide one header block, and what follows it.
    fn block(&mut self, block: &[u8]) -> Result<(), ComputeError> {
        if block.iter().all(|byte| *byte == 0) {
            // The end-of-archive marker. It is kept so the spool stays a valid
            // archive, and headers stay expected in case another follows.
            self.sink.write_all(block)?;
            self.body = Body::Header;
            return Ok(());
        }

        let header = tar::Header::from_byte_slice(block);
        let entry_type = header.entry_type();
        let size = header.size().unwrap_or(0);
        let padded = size.saturating_add(BLOCK as u64 - 1) / BLOCK as u64 * BLOCK as u64;

        // Metadata describes the entry that follows it, so it waits for that
        // entry's decision rather than being written on its own.
        if entry_type.is_pax_local_extensions()
            || entry_type.is_gnu_longname()
            || entry_type.is_gnu_longlink()
        {
            self.pending.extend_from_slice(block);
            self.body = Body::Hold(padded);
            return Ok(());
        }
        // A global header applies to everything that follows it.
        if entry_type.is_pax_global_extensions() {
            self.pending.extend_from_slice(block);
            self.sink.write_all(&self.pending)?;
            self.pending.clear();
            self.body = Body::Copy(padded);
            return Ok(());
        }

        let kept = match self.selector {
            None => true,
            Some(selector) => {
                match header
                    .path()
                    .ok()
                    .and_then(|path| stripped_entry(&path, &self.workspace))
                {
                    // The header's name is what a long name is truncated from,
                    // so a decision drawn from it can only keep more than the
                    // real name would. The metadata held in `pending` carries
                    // the full name to the extractor.
                    Some(relative) => selector
                        .selects(&relative, entry_type.is_dir())
                        .map_err(|error| ComputeError::Config(error.to_string()))?,
                    // The workspace directory itself, which is the destination
                    // rather than something inside it.
                    None => false,
                }
            }
        };

        if kept {
            self.sink.write_all(&self.pending)?;
            self.sink.write_all(block)?;
            self.body = Body::Copy(padded);
        } else {
            self.body = Body::Skip(padded);
        }
        self.pending.clear();

        Ok(())
    }
}

fn extract_tar<R: Read + Seek>(
    dest_path: &Path,
    mut source: R,
    workspace: &str,
) -> Result<(), ComputeError> {
    fs::create_dir_all(dest_path)?;
    let workspace = workspace.trim_matches('/');

    // Validate the whole archive before writing anything, so a refusal cannot
    // leave a half-extracted tree behind.
    let mut entries = 0usize;
    let mut bytes = 0u64;
    {
        let mut archive = Archive::new(&mut source);
        for entry in archive.entries()? {
            let entry = entry?;
            entries += 1;
            bytes = bytes.saturating_add(entry.header().size().unwrap_or(0));
            if entries > EXTRACT_MAX_ENTRIES || bytes > EXTRACT_MAX_BYTES {
                return Err(ComputeError::Config(format!(
                    "the archive exceeds the transfer limit of {EXTRACT_MAX_ENTRIES} entries \
                     or {EXTRACT_MAX_BYTES} bytes"
                )));
            }

            let path = entry.path()?;
            let Some(stripped) = stripped_entry(&path, workspace) else {
                continue;
            };

            // An archive is agent-controlled: only regular files and
            // directories may land on the host, and nothing may resolve above
            // the staging root.
            let entry_type = entry.header().entry_type();
            if !(entry_type.is_file() || entry_type.is_dir()) {
                return Err(ComputeError::Config(format!(
                    "archive contains a non-regular entry: {}",
                    stripped.display()
                )));
            }
            if stripped.is_absolute()
                || stripped
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                return Err(ComputeError::Config(format!(
                    "archive entry escapes the staging root: {}",
                    stripped.display()
                )));
            }
        }
    }

    source.seek(SeekFrom::Start(0))?;
    let mut archive = Archive::new(source);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let Some(stripped) = stripped_entry(&path, workspace) else {
            continue;
        };
        let dest = dest_path.join(stripped);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        entry.unpack(&dest)?;
    }

    Ok(())
}

/// How many entries and bytes a transfer may carry into the host.
const EXTRACT_MAX_ENTRIES: usize = 200_000;
const EXTRACT_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// How long a command may run before it is abandoned.
pub(super) const EXEC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// A provisioning step is setup work, not an interactive shell; it gets a
/// shorter deadline than `EXEC_TIMEOUT`.
pub(super) const PROVISION_STEP_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(10 * 60);

/// How much of one stream is kept before it is truncated.
const EXEC_OUTPUT_CAP: usize = 8 * 1024 * 1024;

fn append_capped(buffer: &mut Vec<u8>, chunk: &[u8], cap: usize) {
    let truncated = buffer.len() >= cap;
    let remaining = cap.saturating_sub(buffer.len());
    if !truncated && remaining > 0 {
        buffer.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    if buffer.len() >= cap && !buffer.ends_with(b"\n[truncated]\n") {
        buffer.extend_from_slice(b"\n[truncated]\n");
    }
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
    fn a_remote_cleartext_endpoint_is_refused() {
        let error = connect_with_host("tcp://127.0.0.1:2375").expect_err("tcp refused");
        assert!(error.to_string().contains("tcp://"), "{error}");

        let error = connect_with_host("ssh://host").expect_err("unknown scheme refused");
        assert!(error.to_string().contains("ssh"), "{error}");
    }

    #[test]
    fn summary_names_drop_the_docker_leading_slash() {
        assert_eq!(
            strip_leading_slash("/litterbox-repo-slug"),
            "litterbox-repo-slug"
        );
        assert_eq!(
            strip_leading_slash("litterbox-repo-slug"),
            "litterbox-repo-slug"
        );
    }

    #[tokio::test]
    #[ignore = "requires a Docker daemon; run with --ignored and LITTERBOX_DOCKER_TESTS=1"]
    async fn docker_connects_and_ensures_image() -> Result<(), Box<dyn std::error::Error>> {
        // Requires a running Docker daemon; opt in with LITTERBOX_DOCKER_TESTS.
        assert!(
            std::env::var("LITTERBOX_DOCKER_TESTS").is_ok(),
            "set LITTERBOX_DOCKER_TESTS=1 and run with --ignored to exercise this test"
        );

        let compute = DockerCompute::connect()?;
        compute.ensure_image("busybox:latest").await?;
        Ok(())
    }
}

#[cfg(test)]
mod transfer_tests {
    use std::fs;
    use std::io::Cursor;
    use std::path::Path;

    use super::{build_tar, extract_tar};

    #[test]
    fn the_archive_buffer_refuses_to_grow_past_the_limit() {
        use std::io::Write;

        let mut buffer = super::CappedBuffer::new(4);
        buffer.write_all(b"four").expect("within the limit");
        let error = buffer.write_all(b"!").expect_err("past the limit");
        assert!(error.to_string().contains("limit"), "{error}");
    }

    #[test]
    fn the_staged_file_refuses_to_grow_past_the_limit() {
        use std::io::Write;

        let file = tempfile::tempfile().expect("temp file");
        let mut capped = super::CappedFile::new(file, 4);
        capped.write_all(b"four").expect("within the limit");
        let error = capped.write_all(b"!").expect_err("past the limit");
        assert!(error.to_string().contains("limit"), "{error}");
    }

    fn append_file(builder: &mut tar::Builder<Vec<u8>>, name: &str, contents: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, name, contents)
            .expect("append file");
    }

    fn append_symlink(builder: &mut tar::Builder<Vec<u8>>, name: &str, target: &str) {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_link(&mut header, name, target)
            .expect("append symlink");
    }

    #[test]
    fn extract_rejects_symlinks_and_writes_nothing() {
        let mut builder = tar::Builder::new(Vec::new());
        append_file(&mut builder, "benign.txt", b"hello");
        append_symlink(&mut builder, "evil", "/etc/passwd");
        let archive = builder.into_inner().expect("finish tar");

        let dir = tempfile::tempdir().expect("tempdir");
        let error =
            extract_tar(dir.path(), Cursor::new(&archive), "/src").expect_err("symlink rejected");
        assert!(
            error.to_string().contains("non-regular"),
            "the refusal names what it rejected: {error}"
        );
        assert!(
            fs::read_dir(dir.path()).expect("read").next().is_none(),
            "validation happens before anything is written"
        );
    }

    #[test]
    fn extraction_strips_the_workspace_the_caller_reports() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "sandbox/README.md", &b"hello"[..])
            .expect("append");
        let archive = builder.into_inner().expect("finish");

        extract_tar(dir.path(), Cursor::new(&archive), "/sandbox").expect("extract");
        assert_eq!(
            fs::read_to_string(dir.path().join("README.md")).expect("read"),
            "hello"
        );
    }

    #[test]
    fn extract_rejects_parent_traversal() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(1);
        header.set_mode(0o644);
        // The builder refuses to write a `..` path, so the name is set raw.
        let name = b"../escape";
        header.as_old_mut().name[..name.len()].copy_from_slice(name);
        header.set_cksum();
        builder
            .append(&header, &b"x"[..])
            .expect("append traversal entry");
        let archive = builder.into_inner().expect("finish tar");

        let dir = tempfile::tempdir().expect("tempdir");
        let error =
            extract_tar(dir.path(), Cursor::new(&archive), "/src").expect_err("traversal rejected");
        assert!(
            error.to_string().contains("escapes"),
            "unexpected error: {error}"
        );
        assert!(!dir.path().parent().unwrap().join("escape").exists());
    }

    /// Rejects the paths a sandbox builds rather than edits.
    struct RejectBuildOutput;

    impl crate::sandbox::contract::SnapshotSelector for RejectBuildOutput {
        fn selects(
            &self,
            relative: &Path,
            _is_dir: bool,
        ) -> Result<bool, crate::sandbox::contract::Error> {
            Ok(!relative.starts_with("target") && !relative.starts_with("node_modules"))
        }
    }

    fn filtered(
        archive: &[u8],
        workspace: &str,
        selector: Option<&dyn crate::sandbox::contract::SnapshotSelector>,
    ) -> Vec<u8> {
        filtered_within(archive, workspace, selector, super::EXTRACT_MAX_BYTES)
    }

    fn filtered_within(
        archive: &[u8],
        workspace: &str,
        selector: Option<&dyn crate::sandbox::contract::SnapshotSelector>,
        limit: u64,
    ) -> Vec<u8> {
        let mut kept =
            super::SelectingArchive::new(super::CappedBuffer::new(limit), workspace, selector);
        // Awkward chunks: a header or an entry that straddles a boundary still
        // has to be read as one.
        for chunk in archive.chunks(7) {
            kept.select(chunk).expect("filter");
        }
        kept.finish().expect("finish");
        kept.into_inner().into_vec()
    }

    #[test]
    fn the_transfer_keeps_only_what_the_selector_wants() {
        let mut builder = tar::Builder::new(Vec::new());
        append_file(&mut builder, "sandbox/README.md", b"hello");
        append_file(&mut builder, "sandbox/target/artifact", b"built");
        let archive = builder.into_inner().expect("finish");

        let kept = filtered(&archive, "/sandbox", Some(&RejectBuildOutput));
        let dir = tempfile::tempdir().expect("tempdir");
        extract_tar(dir.path(), Cursor::new(&kept), "/sandbox").expect("extract");
        assert!(dir.path().join("README.md").is_file());
        assert!(!dir.path().join("target").exists());
    }

    #[test]
    fn a_rejected_link_does_not_abort_the_transfer() {
        let mut builder = tar::Builder::new(Vec::new());
        append_file(&mut builder, "sandbox/README.md", b"hello");
        append_symlink(
            &mut builder,
            "sandbox/node_modules/.bin/tool",
            "/etc/passwd",
        );
        let archive = builder.into_inner().expect("finish");

        let kept = filtered(&archive, "/sandbox", Some(&RejectBuildOutput));
        let dir = tempfile::tempdir().expect("tempdir");
        extract_tar(dir.path(), Cursor::new(&kept), "/sandbox").expect("extract");
        assert!(dir.path().join("README.md").is_file());
        assert!(!dir.path().join("node_modules").exists());
    }

    #[test]
    fn a_rejected_entry_does_not_count_toward_the_limit() {
        let mut builder = tar::Builder::new(Vec::new());
        append_file(&mut builder, "sandbox/README.md", b"hello");
        append_file(&mut builder, "sandbox/target/artifact", &[b'x'; 64 * 1024]);
        let archive = builder.into_inner().expect("finish");

        // Room for what is kept, nowhere near what is rejected.
        let kept = filtered_within(&archive, "/sandbox", Some(&RejectBuildOutput), 16 * 1024);
        let dir = tempfile::tempdir().expect("tempdir");
        extract_tar(dir.path(), Cursor::new(&kept), "/sandbox").expect("extract");
        assert!(dir.path().join("README.md").is_file());
    }

    #[test]
    fn the_transfer_keeps_a_long_path_with_its_metadata() {
        let directory = "d".repeat(150);
        let mut builder = tar::Builder::new(Vec::new());
        append_file(
            &mut builder,
            &format!("sandbox/{directory}/file.txt"),
            b"long",
        );
        let archive = builder.into_inner().expect("finish");

        let kept = filtered(&archive, "/sandbox", Some(&RejectBuildOutput));
        let dir = tempfile::tempdir().expect("tempdir");
        extract_tar(dir.path(), Cursor::new(&kept), "/sandbox").expect("extract");
        assert!(dir.path().join(&directory).join("file.txt").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn upload_skips_symlinks_without_reading_the_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("real.txt"), "data").expect("write");
        std::os::unix::fs::symlink("/etc/passwd", dir.path().join("link")).expect("symlink");

        let archive = build_tar(dir.path()).expect("build tar");
        let mut reader = tar::Archive::new(std::io::Cursor::new(archive));
        let names: Vec<String> = reader
            .entries()
            .expect("entries")
            .map(|entry| {
                entry
                    .expect("entry")
                    .path()
                    .expect("path")
                    .display()
                    .to_string()
            })
            .collect();

        assert!(names.iter().any(|name| name.ends_with("real.txt")));
        assert!(!names.iter().any(|name| name.ends_with("link")));
    }
}
