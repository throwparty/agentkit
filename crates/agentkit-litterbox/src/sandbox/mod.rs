use std::collections::HashMap;
use std::io::Cursor;
use std::net::TcpListener;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::future::BoxFuture;
use tar::Archive;
use tempfile::TempDir;
use tokio::time::sleep;

use crate::compute::{Compute, ContainerInspection, ContainerSpec};
use crate::domain::{
    slugify_name,
    ComputeError,
    ExecutionResult,
    ForwardedPortMapping,
    SandboxConfig,
    SandboxError,
    SandboxMetadata,
    SandboxStatus,
};
use crate::scm::Scm;

const DEFAULT_WORKDIR: &str = "/src";
const DEFAULT_PORT_RANGE_START: u16 = 3000;
const DEFAULT_PORT_RANGE_END: u16 = 8000;
const PORT_ALLOC_BACKOFF_MS: u64 = 25;
const PORT_ALLOC_MAX_RETRIES: usize = 32;

pub trait SandboxProvider {
    fn create<'a>(
        &'a self,
        name: &'a str,
        config: &'a SandboxConfig,
    ) -> BoxFuture<'a, Result<SandboxMetadata, SandboxError>>;
    fn inspect_container<'a>(
        &'a self,
        container_id: &'a str,
    ) -> BoxFuture<'a, Result<ContainerInspection, SandboxError>>;
    fn pause<'a>(&'a self, container_id: &'a str)
        -> BoxFuture<'a, Result<(), SandboxError>>;
    fn resume<'a>(&'a self, container_id: &'a str)
        -> BoxFuture<'a, Result<(), SandboxError>>;
    fn delete<'a>(&'a self, metadata: &'a SandboxMetadata)
        -> BoxFuture<'a, Result<(), SandboxError>>;
    fn shell<'a>(
        &'a self,
        metadata: &'a SandboxMetadata,
        command: &'a [String],
    ) -> BoxFuture<'a, Result<ExecutionResult, SandboxError>>;
    fn upload_path<'a>(
        &'a self,
        metadata: &'a SandboxMetadata,
        src_path: &'a Path,
        dest_path: &'a str,
    ) -> BoxFuture<'a, Result<(), SandboxError>>;
    fn download_path<'a>(
        &'a self,
        metadata: &'a SandboxMetadata,
        src_path: &'a str,
        dest_path: &'a Path,
    ) -> BoxFuture<'a, Result<(), SandboxError>>;
}

pub struct DockerSandboxProvider<S, C> {
    scm: S,
    compute: C,
}

impl<S, C> DockerSandboxProvider<S, C> {
    pub fn new(scm: S, compute: C) -> Self {
        Self { scm, compute }
    }
}

pub fn container_name_for_slug(repo_prefix: &str, slug: &str) -> String {
    format!("litterbox-{}-{}", repo_prefix, slug)
}

pub fn branch_name_for_slug(slug: &str) -> String {
    format!("litterbox/{}", slug)
}

impl<S, C> SandboxProvider for DockerSandboxProvider<S, C>
where
    S: Scm + Send + Sync,
    C: Compute + Send + Sync,
{
    fn create<'a>(
        &'a self,
        name: &'a str,
        config: &'a SandboxConfig,
    ) -> BoxFuture<'a, Result<SandboxMetadata, SandboxError>> {
        Box::pin(async move {
            let slug = slugify_name(name)?;
            let branch_name = self.scm.create_branch(&slug)?;
            let repo_prefix = self.scm.repo_prefix()?;
            let archive = match self.scm.make_archive("HEAD") {
                Ok(archive) => archive,
                Err(error) => {
                    let _ = self.scm.delete_branch(&slug);
                    return Err(error);
                }
            };
            let staged = match stage_archive(&archive) {
                Ok(staged) => staged,
                Err(error) => {
                    let _ = self.scm.delete_branch(&slug);
                    return Err(error);
                }
            };

            if let Err(error) = self.compute.ensure_image(&config.image).await {
                let _ = self.scm.delete_branch(&slug);
                return Err(error);
            }

            let (env, port_bindings, forwarded_ports) =
                build_forwarded_ports(config).await?;

            let spec = ContainerSpec {
                name: container_name_for_slug(&repo_prefix, &slug),
                image: config.image.clone(),
                command: vec!["sh".to_string(), "-c".to_string(), "tail -f /dev/null".to_string()],
                working_dir: Some(DEFAULT_WORKDIR.to_string()),
                env,
                port_bindings,
            };

            let container_id = match self.compute.create_container(&spec).await {
                Ok(id) => id,
                Err(error) => {
                    let _ = self.scm.delete_branch(&slug);
                    if is_container_name_conflict(&error) {
                        return Err(SandboxError::SandboxExists { name: slug.clone() });
                    }
                    return Err(error);
                }
            };

            if let Err(error) = self
                .compute
                .upload_path(&container_id, staged.path(), DEFAULT_WORKDIR)
                .await
            {
                let _ = self.compute.delete_container(&container_id).await;
                let _ = self.scm.delete_branch(&slug);
                return Err(error);
            }

            if let Some(command) = &config.setup_command {
                let startup_command = vec!["sh".to_string(), "-c".to_string(), command.clone()];
                let result = match self
                    .compute
                    .exec(&container_id, &startup_command, Some(DEFAULT_WORKDIR))
                    .await
                {
                    Ok(result) => result,
                    Err(error) => {
                        let _ = self.compute.delete_container(&container_id).await;
                        let _ = self.scm.delete_branch(&slug);
                        return Err(error);
                    }
                };

                if result.exit_code != 0 {
                    let _ = self.compute.delete_container(&container_id).await;
                    let _ = self.scm.delete_branch(&slug);
                    let stderr = if result.stderr.is_empty() {
                        result.stdout
                    } else {
                        result.stderr
                    };
                    return Err(SandboxError::SetupCommandFailed {
                        exit_code: result.exit_code,
                        stderr,
                    });
                }
            }

            Ok(SandboxMetadata {
                name: slug,
                branch_name,
                container_id,
                status: SandboxStatus::Active,
                forwarded_ports,
            })
        })
    }

    fn pause<'a>(
        &'a self,
        container_id: &'a str,
    ) -> BoxFuture<'a, Result<(), SandboxError>> {
        Box::pin(async move { self.compute.pause_container(container_id).await })
    }

    fn inspect_container<'a>(
        &'a self,
        container_id: &'a str,
    ) -> BoxFuture<'a, Result<ContainerInspection, SandboxError>> {
        Box::pin(async move { self.compute.inspect_container(container_id).await })
    }

    fn resume<'a>(
        &'a self,
        container_id: &'a str,
    ) -> BoxFuture<'a, Result<(), SandboxError>> {
        Box::pin(async move { self.compute.resume_container(container_id).await })
    }

    fn delete<'a>(
        &'a self,
        metadata: &'a SandboxMetadata,
    ) -> BoxFuture<'a, Result<(), SandboxError>> {
        Box::pin(async move {
            self.compute.delete_container(&metadata.container_id).await?;
            self.scm.delete_branch(&metadata.name)?;
            Ok(())
        })
    }

    fn shell<'a>(
        &'a self,
        metadata: &'a SandboxMetadata,
        command: &'a [String],
    ) -> BoxFuture<'a, Result<ExecutionResult, SandboxError>> {
        Box::pin(async move {
            self.compute
                .exec(&metadata.container_id, command, Some(DEFAULT_WORKDIR))
                .await
        })
    }

    fn upload_path<'a>(
        &'a self,
        metadata: &'a SandboxMetadata,
        src_path: &'a Path,
        dest_path: &'a str,
    ) -> BoxFuture<'a, Result<(), SandboxError>> {
        Box::pin(async move {
            self.compute
                .upload_path(&metadata.container_id, src_path, dest_path)
                .await
        })
    }

    fn download_path<'a>(
        &'a self,
        metadata: &'a SandboxMetadata,
        src_path: &'a str,
        dest_path: &'a Path,
    ) -> BoxFuture<'a, Result<(), SandboxError>> {
        Box::pin(async move {
            self.compute
                .download_path(&metadata.container_id, src_path, dest_path)
                .await
        })
    }
}

fn stage_archive(archive: &[u8]) -> Result<TempDir, SandboxError> {
    let tempdir = TempDir::new()?;
    let mut archive = Archive::new(Cursor::new(archive));
    archive.unpack(tempdir.path())?;
    Ok(tempdir)
}

fn is_container_name_conflict(error: &SandboxError) -> bool {
    matches!(
        error,
        SandboxError::Compute(ComputeError::ContainerProvision {
            source: bollard::errors::Error::DockerResponseServerError { status_code: 409, .. }
        })
    )
}

async fn build_forwarded_ports(
    config: &SandboxConfig,
) -> Result<(Vec<String>, HashMap<String, Vec<bollard::models::PortBinding>>, Vec<ForwardedPortMapping>), SandboxError> {
    if config.forwarded_ports.is_empty() {
        return Ok((Vec::new(), HashMap::new(), Vec::new()));
    }

    let mut env = Vec::new();
    let mut port_bindings: HashMap<String, Vec<bollard::models::PortBinding>> = HashMap::new();
    let mut forwarded = Vec::new();

    for port in &config.forwarded_ports {
        let slug = slugify_name(&port.name)?;
        let env_key = env_var_for_slug(&slug);
        let host_port = allocate_host_port(DEFAULT_PORT_RANGE_START, DEFAULT_PORT_RANGE_END).await?;
        env.push(format!("{env_key}={host_port}"));
        port_bindings.insert(
            format!("{}/tcp", port.target),
            vec![bollard::models::PortBinding {
                host_ip: Some("0.0.0.0".to_string()),
                host_port: Some(host_port.to_string()),
            }],
        );
        forwarded.push(ForwardedPortMapping {
            name: port.name.clone(),
            target: port.target,
            host_port,
            env_var: env_key,
        });
    }

    Ok((env, port_bindings, forwarded))
}

fn env_var_for_slug(slug: &str) -> String {
    format!(
        "LITTERBOX_FWD_PORT_{}",
        slug.replace('-', "_").to_ascii_uppercase()
    )
}

async fn allocate_host_port(range_start: u16, range_end: u16) -> Result<u16, SandboxError> {
    if range_end < range_start {
        return Err(SandboxError::Config(format!(
            "Invalid port range: {range_start}-{range_end}"
        )));
    }

    let range = (range_end - range_start + 1) as u64;
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| SandboxError::Config(error.to_string()))?
        .as_nanos() as u64;
    let max_attempts = PORT_ALLOC_MAX_RETRIES.min(range as usize);

    for attempt in 0..max_attempts {
        let offset = (seed + attempt as u64) % range;
        let candidate = range_start + offset as u16;
        if TcpListener::bind(("127.0.0.1", candidate)).is_ok() {
            return Ok(candidate);
        }
        sleep(Duration::from_millis(PORT_ALLOC_BACKOFF_MS)).await;
    }

    Err(SandboxError::Config(format!(
        "No available host ports in range {range_start}-{range_end}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use bollard::query_parameters::RemoveContainerOptions;
    use git2::{IndexAddOption, Repository, Signature};
    use tempfile::TempDir;

    use crate::compute::DockerCompute;
    use crate::domain::ForwardedPort;
    use crate::scm::ThreadSafeScm;

    static UNIQUE_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn unique_suffix() -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let counter = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{nanos}-{counter}")
    }

    fn init_repo() -> (TempDir, Repository) {
        let tempdir = TempDir::new().expect("tempdir");
        let repo = Repository::init(tempdir.path()).expect("repo init");

        let file_path = tempdir.path().join("README.md");
        fs::write(&file_path, "hello").expect("write file");

        let mut index = repo.index().expect("index");
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .expect("add all");
        index.write().expect("index write");
        let tree_id = index.write_tree().expect("write tree");

        let signature = Signature::now("Litterbox", "noreply@example.com")
            .expect("signature");
        {
            let tree = repo.find_tree(tree_id).expect("find tree");
            repo.commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])
                .expect("commit");
        }

        (tempdir, repo)
    }

    #[test]
    fn env_var_for_slug_formats_name() {
        let env = env_var_for_slug("my-service");

        assert_eq!(env, "LITTERBOX_FWD_PORT_MY_SERVICE");
    }

    #[tokio::test]
    async fn allocate_host_port_returns_in_range() {
        let port = allocate_host_port(45000, 45010).await.expect("alloc port");

        assert!((45000..=45010).contains(&port));
    }

    #[tokio::test]
    async fn allocate_host_port_skips_bound_port() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind port");
        let port = listener.local_addr().expect("local addr").port();

        if TcpListener::bind(("127.0.0.1", port + 1)).is_err() {
            return;
        }

        let allocated = allocate_host_port(port, port + 1)
            .await
            .expect("alloc port");

        assert_ne!(allocated, port);
    }

    #[tokio::test]
    async fn allocate_host_port_rejects_invalid_range() {
        let err = allocate_host_port(9000, 8000)
            .await
            .expect_err("invalid range rejected");

        assert!(err.to_string().contains("Invalid port range"));
    }

    #[tokio::test]
    async fn allocate_host_port_fails_when_range_exhausted() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind port");
        let port = listener.local_addr().expect("local addr").port();

        let err = allocate_host_port(port, port)
            .await
            .expect_err("no available ports");

        assert!(err.to_string().contains("No available host ports"));
    }

    #[tokio::test]
    async fn build_forwarded_ports_returns_env_and_mappings() {
        let config = SandboxConfig {
            image: "busybox".to_string(),
            setup_command: None,
            forwarded_ports: vec![ForwardedPort {
                name: "web".to_string(),
                target: 8080,
            }],
        };

        let (env, port_bindings, forwarded) =
            build_forwarded_ports(&config).await.expect("build ports");

        assert_eq!(env.len(), 1);
        assert!(env[0].starts_with("LITTERBOX_FWD_PORT_WEB="));
        assert!(port_bindings.contains_key("8080/tcp"));
        assert_eq!(forwarded.len(), 1);
        assert_eq!(forwarded[0].env_var, "LITTERBOX_FWD_PORT_WEB");
        assert_eq!(forwarded[0].target, 8080);
        assert!((DEFAULT_PORT_RANGE_START..=DEFAULT_PORT_RANGE_END).contains(&forwarded[0].host_port));
    }

    #[tokio::test]
    async fn build_forwarded_ports_allows_empty_config() {
        let config = SandboxConfig {
            image: "busybox".to_string(),
            setup_command: None,
            forwarded_ports: Vec::new(),
        };

        let (env, port_bindings, forwarded) =
            build_forwarded_ports(&config).await.expect("build ports");

        assert!(env.is_empty());
        assert!(port_bindings.is_empty());
        assert!(forwarded.is_empty());
    }

    #[tokio::test]
    async fn build_forwarded_ports_rejects_invalid_name() {
        let config = SandboxConfig {
            image: "busybox".to_string(),
            setup_command: None,
            forwarded_ports: vec![ForwardedPort {
                name: "----".to_string(),
                target: 8080,
            }],
        };

        let err = build_forwarded_ports(&config)
            .await
            .expect_err("invalid name rejected");

        assert!(err.to_string().contains("Invalid sandbox name"));
    }

    #[tokio::test]
    async fn create_provisions_container() -> Result<(), Box<dyn std::error::Error>> {
        if std::env::var("LITTERBOX_DOCKER_TESTS").is_err() {
            return Ok(());
        }

        let (tempdir, _repo) = init_repo();
        let scm = ThreadSafeScm::open(tempdir.path())?;
        let compute = DockerCompute::connect()?;
        let provider = DockerSandboxProvider::new(scm, compute);

        let name = format!("sandbox-{}", unique_suffix());
        let metadata = provider
            .create(
                &name,
                &SandboxConfig {
                    image: "busybox:latest".to_string(),
                    setup_command: None,
                    forwarded_ports: Vec::new(),
                },
            )
            .await?;

        let client = provider.compute.client();
        let container = client.inspect_container(&metadata.container_id, None).await?;
        let running = container
            .state
            .and_then(|state| state.running)
            .unwrap_or(false);
        assert!(running);

        let _ = client
            .remove_container(
                &metadata.container_id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;
        let _ = provider.scm.delete_branch(&metadata.name);

        Ok(())
    }

    #[tokio::test]
    async fn create_provisions_forwarded_ports() -> Result<(), Box<dyn std::error::Error>> {
        if std::env::var("LITTERBOX_DOCKER_TESTS").is_err() {
            return Ok(());
        }

        let (tempdir, _repo) = init_repo();
        let scm = ThreadSafeScm::open(tempdir.path())?;
        let compute = DockerCompute::connect()?;
        let provider = DockerSandboxProvider::new(scm, compute);

        let name = format!("sandbox-{}", unique_suffix());
        let metadata = provider
            .create(
                &name,
                &SandboxConfig {
                    image: "busybox:latest".to_string(),
                    setup_command: None,
                    forwarded_ports: vec![ForwardedPort {
                        name: "web".to_string(),
                        target: 8080,
                    }],
                },
            )
            .await?;

        let client = provider.compute.client();
        let container = client.inspect_container(&metadata.container_id, None).await?;
        let env = container
            .config
            .and_then(|config| config.env)
            .unwrap_or_default();
        let env_value = env
            .iter()
            .find(|entry| entry.starts_with("LITTERBOX_FWD_PORT_WEB="))
            .expect("env var present")
            .split('=' )
            .nth(1)
            .expect("env var value");

        let host_port = env_value.parse::<u16>().expect("host port parses");
        assert!((DEFAULT_PORT_RANGE_START..=DEFAULT_PORT_RANGE_END).contains(&host_port));

        let bindings = container
            .host_config
            .and_then(|config| config.port_bindings)
            .expect("port bindings present");
        let binding = bindings
            .get("8080/tcp")
            .and_then(|entry| entry.as_ref())
            .and_then(|entries| entries.first())
            .expect("port binding present");
        assert_eq!(binding.host_port.as_deref(), Some(env_value));

        let _ = client
            .remove_container(
                &metadata.container_id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;
        let _ = provider.scm.delete_branch(&metadata.name);

        Ok(())
    }

    #[tokio::test]
    async fn pause_resume_delete_container() -> Result<(), Box<dyn std::error::Error>> {
        if std::env::var("LITTERBOX_DOCKER_TESTS").is_err() {
            return Ok(());
        }

        let (tempdir, _repo) = init_repo();
        let scm = ThreadSafeScm::open(tempdir.path())?;
        let compute = DockerCompute::connect()?;
        let provider = DockerSandboxProvider::new(scm, compute);

        let name = format!("sandbox-{}", unique_suffix());
        let metadata = provider
            .create(
                &name,
                &SandboxConfig {
                    image: "busybox:latest".to_string(),
                    setup_command: None,
                    forwarded_ports: Vec::new(),
                },
            )
            .await?;

        provider.pause(&metadata.container_id).await?;
        let client = provider.compute.client();
        let container = client.inspect_container(&metadata.container_id, None).await?;
        let paused = container
            .state
            .and_then(|state| state.paused)
            .unwrap_or(false);
        assert!(paused);

        provider.resume(&metadata.container_id).await?;
        let container = client.inspect_container(&metadata.container_id, None).await?;
        let running = container
            .state
            .and_then(|state| state.running)
            .unwrap_or(false);
        assert!(running);

        provider.delete(&metadata).await?;
        assert!(client.inspect_container(&metadata.container_id, None).await.is_err());

        let repo = Repository::open(tempdir.path())?;
        assert!(repo
            .find_branch(&metadata.branch_name, git2::BranchType::Local)
            .is_err());

        Ok(())
    }

    #[tokio::test]
    async fn shell_executes_commands() -> Result<(), Box<dyn std::error::Error>> {
        if std::env::var("LITTERBOX_DOCKER_TESTS").is_err() {
            return Ok(());
        }

        let (tempdir, _repo) = init_repo();
        let scm = ThreadSafeScm::open(tempdir.path())?;
        let compute = DockerCompute::connect()?;
        let provider = DockerSandboxProvider::new(scm, compute);

        let name = format!("sandbox-{}", unique_suffix());
        let metadata = provider
            .create(
                &name,
                &SandboxConfig {
                    image: "busybox:latest".to_string(),
                    setup_command: None,
                    forwarded_ports: Vec::new(),
                },
            )
            .await?;

        let result = provider
            .shell(
                &metadata,
                &[
                    "sh".to_string(),
                    "-c".to_string(),
                    "echo hello".to_string(),
                ],
            )
            .await?;
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("hello"));

        let failure = provider
            .shell(
                &metadata,
                &[
                    "sh".to_string(),
                    "-c".to_string(),
                    "ls /does-not-exist".to_string(),
                ],
            )
            .await?;
        assert_ne!(failure.exit_code, 0);
        assert!(!failure.stderr.is_empty());

        provider.delete(&metadata).await?;
        Ok(())
    }

}
