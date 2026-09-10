use async_trait::async_trait;
use docker_wrapper::{
    CpCommand, CreateCommand, DockerCommand, ExecCommand, GenericCommand, PullCommand, RmCommand,
    StartCommand, StopCommand,
};
use serde_json::Value;
use std::io;
use std::path::Path;

use crate::{ContainerSpec, ContainerSummary, DockerClient, DockerResult, ExecOutput};

pub struct DockerWrapperClient;

impl DockerWrapperClient {
    pub fn new() -> Self {
        Self
    }

    fn parse_container_names(raw: &str) -> Vec<String> {
        raw.split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| {
                if name.starts_with('/') {
                    name.to_string()
                } else {
                    format!("/{name}")
                }
            })
            .collect()
    }
}

#[async_trait]
impl DockerClient for DockerWrapperClient {
    async fn pull_image(&self, image: &str) -> DockerResult<()> {
        PullCommand::new(image).execute().await?;
        Ok(())
    }

    async fn create_container(&self, spec: ContainerSpec) -> DockerResult<String> {
        let mut command = CreateCommand::new(spec.image);
        if let Some(name) = spec.name {
            command = command.name(name);
        }
        if !spec.cmd.is_empty() {
            command = command.cmd(spec.cmd);
        }
        let result = command.run().await?;
        Ok(result.container_id().to_string())
    }

    async fn start_container(&self, container_id: &str) -> DockerResult<()> {
        StartCommand::new(container_id).execute().await?;
        Ok(())
    }

    async fn stop_container(&self, container_id: &str) -> DockerResult<()> {
        StopCommand::new(container_id).execute().await?;
        Ok(())
    }

    async fn remove_container(&self, container_id: &str, force: bool) -> DockerResult<()> {
        let mut command = RmCommand::new(container_id);
        if force {
            command = command.force();
        }
        command.execute().await?;
        Ok(())
    }

    async fn list_containers(&self, all: bool) -> DockerResult<Vec<ContainerSummary>> {
        let mut command = GenericCommand::new("ps");
        if all {
            command = command.arg("--all");
        }
        let output = command
            .arg("--no-trunc")
            .arg("--format")
            .arg("{{json .}}")
            .execute()
            .await?;

        let mut containers = Vec::new();
        for line in output.stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(line).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("failed to parse docker ps output: {err}; line: {line}"),
                )
            })?;

            let id = value
                .get("ID")
                .and_then(Value::as_str)
                .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "docker ps output missing ID"))?
                .to_string();

            let names = value
                .get("Names")
                .and_then(Value::as_str)
                .map(Self::parse_container_names)
                .unwrap_or_default();

            let status = value
                .get("Status")
                .and_then(Value::as_str)
                .map(str::to_string);

            containers.push(ContainerSummary { id, names, status });
        }

        Ok(containers)
    }

    async fn exec(&self, container_id: &str, cmd: Vec<String>) -> DockerResult<ExecOutput> {
        let output = ExecCommand::new(container_id, cmd).execute().await?;
        Ok(ExecOutput {
            stdout: output.stdout.into_bytes(),
            stderr: output.stderr.into_bytes(),
        })
    }

    async fn copy_file_into(
        &self,
        container_id: &str,
        host_path: &Path,
        container_path: &Path,
    ) -> DockerResult<()> {
        CpCommand::from_host(host_path)
            .to_container(container_id, container_path.to_string_lossy())
            .run()
            .await?;
        Ok(())
    }

    async fn copy_file_out(
        &self,
        container_id: &str,
        container_path: &Path,
        host_path: &Path,
    ) -> DockerResult<()> {
        CpCommand::from_container(container_id, container_path.to_string_lossy())
            .to_host(host_path)
            .run()
            .await?;
        Ok(())
    }
}
