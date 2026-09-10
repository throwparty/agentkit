use async_trait::async_trait;
use bollard::Docker;
use bollard::body_full;
use bollard::container::LogOutput;
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::models::ContainerCreateBody;
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, DownloadFromContainerOptionsBuilder,
    ListContainersOptionsBuilder, RemoveContainerOptionsBuilder, StopContainerOptionsBuilder,
    UploadToContainerOptionsBuilder,
};
use futures::StreamExt;
use std::fs::File;
use std::io;
use std::path::Path;
use tar::{Archive, Builder};

use crate::{ContainerSpec, ContainerSummary, DockerClient, DockerResult, ExecOutput};

pub struct BollardClient {
    docker: Docker,
}

impl BollardClient {
    pub fn new(docker: Docker) -> Self {
        Self { docker }
    }

    pub fn connect_with_local_defaults() -> DockerResult<Self> {
        Ok(Self::new(Docker::connect_with_local_defaults()?))
    }
}

#[async_trait]
impl DockerClient for BollardClient {
    async fn pull_image(&self, image: &str) -> DockerResult<()> {
        let options = Some(
            CreateImageOptionsBuilder::default()
                .from_image(image)
                .build(),
        );
        let mut stream = self.docker.create_image(options, None, None);
        while let Some(msg) = stream.next().await {
            msg?;
        }
        Ok(())
    }

    async fn create_container(&self, spec: ContainerSpec) -> DockerResult<String> {
        let options = spec
            .name
            .as_ref()
            .map(|name| CreateContainerOptionsBuilder::default().name(name).build());

        let config = ContainerCreateBody {
            image: Some(spec.image),
            cmd: if spec.cmd.is_empty() {
                None
            } else {
                Some(spec.cmd)
            },
            ..Default::default()
        };

        let response = self.docker.create_container(options, config).await?;
        Ok(response.id)
    }

    async fn start_container(&self, container_id: &str) -> DockerResult<()> {
        self.docker.start_container(container_id, None).await?;
        Ok(())
    }

    async fn stop_container(&self, container_id: &str) -> DockerResult<()> {
        let options = Some(StopContainerOptionsBuilder::default().t(30).build());
        self.docker.stop_container(container_id, options).await?;
        Ok(())
    }

    async fn remove_container(&self, container_id: &str, force: bool) -> DockerResult<()> {
        let options = Some(
            RemoveContainerOptionsBuilder::default()
                .force(force)
                .build(),
        );
        self.docker.remove_container(container_id, options).await?;
        Ok(())
    }

    async fn list_containers(&self, all: bool) -> DockerResult<Vec<ContainerSummary>> {
        let options = Some(ListContainersOptionsBuilder::default().all(all).build());
        let containers = self.docker.list_containers(options).await?;
        Ok(containers
            .into_iter()
            .map(|container| ContainerSummary {
                id: container.id.unwrap_or_default(),
                names: container.names.unwrap_or_default(),
                status: container.status,
            })
            .collect())
    }

    async fn exec(&self, container_id: &str, cmd: Vec<String>) -> DockerResult<ExecOutput> {
        let exec_options = CreateExecOptions {
            cmd: Some(cmd),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            ..Default::default()
        };
        let exec_id = self
            .docker
            .create_exec(container_id, exec_options)
            .await?
            .id;
        let exec_result = self.docker.start_exec(&exec_id, None).await?;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        if let StartExecResults::Attached { mut output, .. } = exec_result {
            while let Some(log) = output.next().await {
                let log = log?;
                match log {
                    LogOutput::StdErr { message } => stderr.extend_from_slice(message.as_ref()),
                    LogOutput::StdOut { message } => stdout.extend_from_slice(message.as_ref()),
                    LogOutput::StdIn { message } => stdout.extend_from_slice(message.as_ref()),
                    LogOutput::Console { message } => stdout.extend_from_slice(message.as_ref()),
                }
            }
        }

        Ok(ExecOutput { stdout, stderr })
    }

    async fn copy_file_into(
        &self,
        container_id: &str,
        host_path: &Path,
        container_path: &Path,
    ) -> DockerResult<()> {
        let filename = container_path
            .file_name()
            .ok_or_else(|| "container path must include file name")?;
        let parent = container_path.parent().unwrap_or_else(|| Path::new("/"));

        let mut tar_builder = Builder::new(Vec::new());
        let mut file_to_tar = File::open(host_path)?;
        tar_builder.append_file(Path::new(filename), &mut file_to_tar)?;
        let tar_data = tar_builder.into_inner()?;

        let options = UploadToContainerOptionsBuilder::default()
            .path(parent.to_string_lossy().as_ref())
            .build();

        self.docker
            .upload_to_container(container_id, Some(options), body_full(tar_data.into()))
            .await?;

        Ok(())
    }

    async fn copy_file_out(
        &self,
        container_id: &str,
        container_path: &Path,
        host_path: &Path,
    ) -> DockerResult<()> {
        let options = Some(
            DownloadFromContainerOptionsBuilder::default()
                .path(container_path.to_string_lossy().as_ref())
                .build(),
        );
        let mut archive_stream = self.docker.download_from_container(container_id, options);
        let mut tar_bytes = Vec::new();
        while let Some(chunk) = archive_stream.next().await {
            let chunk = chunk?;
            tar_bytes.extend_from_slice(chunk.as_ref());
        }

        if let Some(parent) = host_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut archive = Archive::new(io::Cursor::new(tar_bytes));
        let mut entries = archive.entries()?;
        let mut entry = entries
            .next()
            .ok_or_else(|| "container archive was empty")??;
        let mut output = File::create(host_path)?;
        io::copy(&mut entry, &mut output)?;

        Ok(())
    }
}
