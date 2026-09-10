use bollard_poc::{
    BollardClient, ContainerSpec, DockerClient, DockerResult, DockerWrapperClient, RsDockerClient,
};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const IMAGE_NAME: &str = "busybox:latest";
const HOST_FILE_NAME: &str = "host_file.txt";
const CONTAINER_FILE_PATH: &str = "/tmp/container_file.txt";
const OUTPUT_FILE_NAME: &str = "extracted_container_file.txt";

fn unique_suffix() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let pid = std::process::id();
    format!("{}_{}", millis, pid)
}

fn failure(message: &str) -> Box<dyn std::error::Error + Send + Sync> {
    io::Error::new(io::ErrorKind::Other, message).into()
}

fn temp_dir(prefix: &str) -> DockerResult<PathBuf> {
    let dir = std::env::temp_dir().join(format!("{}_{}", prefix, unique_suffix()));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

async fn create_running_container(
    client: &dyn DockerClient,
    name_prefix: &str,
) -> DockerResult<(String, String)> {
    client.pull_image(IMAGE_NAME).await?;

    let container_name = format!("{}_{}", name_prefix, unique_suffix());
    let spec = ContainerSpec {
        image: IMAGE_NAME.to_string(),
        name: Some(container_name.clone()),
        cmd: vec![
            "tail".to_string(),
            "-f".to_string(),
            "/dev/null".to_string(),
        ],
    };

    let container_id = client.create_container(spec).await?;
    client.start_container(&container_id).await?;
    Ok((container_id, container_name))
}

async fn cleanup_container(client: &dyn DockerClient, container_id: &str) {
    let _ = client.stop_container(container_id).await;
    let _ = client.remove_container(container_id, true).await;
}

fn bollard_client() -> DockerResult<Box<dyn DockerClient>> {
    Ok(Box::new(BollardClient::connect_with_local_defaults()?))
}

fn rs_docker_client() -> DockerResult<Box<dyn DockerClient>> {
    Ok(Box::new(RsDockerClient::connect_with_env()?))
}

fn docker_wrapper_client() -> DockerResult<Box<dyn DockerClient>> {
    Ok(Box::new(DockerWrapperClient::new()))
}

macro_rules! docker_client_test_suite {
    ($module:ident, $client_fn:path $(, skip = $reason:literal)? ) => {
        mod $module {
            use super::*;

            fn client() -> DockerResult<Box<dyn DockerClient>> {
                $client_fn()
            }

            #[tokio::test]
            $(#[ignore = $reason])?
            async fn pull_image() -> DockerResult<()> {
                let client = client()?;
                client.pull_image(IMAGE_NAME).await
            }

            #[tokio::test]
            $(#[ignore = $reason])?
            async fn list_containers() -> DockerResult<()> {
                let client = client()?;
                let client_ref = client.as_ref();
                let (container_id, container_name) =
                    create_running_container(client_ref, "docker_client_list").await?;

                let result = async {
                    let containers = client_ref.list_containers(true).await?;
                    let listed = containers
                        .iter()
                        .any(|container| container.names.contains(&format!("/{}", container_name)));
                    if !listed {
                        return Err(failure("container was not found in list_containers"));
                    }
                    Ok(())
                }
                .await;

                cleanup_container(client_ref, &container_id).await;
                result
            }

            #[tokio::test]
            $(#[ignore = $reason])?
            async fn exec_command() -> DockerResult<()> {
                let client = client()?;
                let client_ref = client.as_ref();
                let (container_id, _) =
                    create_running_container(client_ref, "docker_client_exec").await?;

                let result = async {
                    let exec_output = client_ref
                        .exec(
                            &container_id,
                            vec![
                                "sh".to_string(),
                                "-c".to_string(),
                                "echo -n hello".to_string(),
                            ],
                        )
                        .await?;
                    let output = String::from_utf8_lossy(&exec_output.stdout);
                    if output != "hello" {
                        return Err(failure("exec output did not match expected value"));
                    }
                    Ok(())
                }
                .await;

                cleanup_container(client_ref, &container_id).await;
                result
            }

            #[tokio::test]
            $(#[ignore = $reason])?
            async fn copy_in_and_out() -> DockerResult<()> {
                let client = client()?;
                let client_ref = client.as_ref();
                let (container_id, _) =
                    create_running_container(client_ref, "docker_client_copy").await?;

                let result = async {
                    let work_dir = temp_dir("docker_client_copy")?;
                    let host_file_path = work_dir.join(HOST_FILE_NAME);
                    let output_file_path = work_dir.join(OUTPUT_FILE_NAME);

                    let mut host_file = File::create(&host_file_path)?;
                    writeln!(host_file, "This is a test file from the host.")?;
                    host_file.flush()?;

                    client_ref
                        .copy_file_into(
                            &container_id,
                            host_file_path.as_path(),
                            Path::new(CONTAINER_FILE_PATH),
                        )
                        .await?;

                    let exec_output = client_ref
                        .exec(
                            &container_id,
                            vec!["cat".to_string(), CONTAINER_FILE_PATH.to_string()],
                        )
                        .await?;
                    let exec_text = String::from_utf8_lossy(&exec_output.stdout);
                    if !exec_text.contains("This is a test file from the host.") {
                        return Err(failure("exec output did not contain expected content"));
                    }

                    client_ref
                        .copy_file_out(
                            &container_id,
                            Path::new(CONTAINER_FILE_PATH),
                            output_file_path.as_path(),
                        )
                        .await?;

                    let mut extracted = String::new();
                    File::open(&output_file_path)?.read_to_string(&mut extracted)?;
                    if !extracted.contains("This is a test file from the host.") {
                        return Err(failure("copied-out file content mismatch"));
                    }

                    fs::remove_dir_all(&work_dir)?;
                    Ok(())
                }
                .await;

                cleanup_container(client_ref, &container_id).await;
                result
            }

            #[tokio::test]
            $(#[ignore = $reason])?
            async fn stop_and_remove() -> DockerResult<()> {
                let client = client()?;
                let client_ref = client.as_ref();
                let (container_id, _) =
                    create_running_container(client_ref, "docker_client_remove").await?;

                let result = async {
                    client_ref.stop_container(&container_id).await?;
                    client_ref.remove_container(&container_id, true).await?;

                    let containers = client_ref.list_containers(true).await?;
                    let still_present =
                        containers.iter().any(|container| container.id == container_id);
                    if still_present {
                        return Err(failure("container still present after removal"));
                    }
                    Ok(())
                }
                .await;

                cleanup_container(client_ref, &container_id).await;
                result
            }

            #[tokio::test]
            $(#[ignore = $reason])?
            async fn remove_missing_container() -> DockerResult<()> {
                let client = client()?;
                let client_ref = client.as_ref();
                let missing_container = "non_existent_container_12345";
                if client_ref.remove_container(missing_container, false).await.is_ok() {
                    return Err(failure(
                        "expected remove_container to fail for non-existent container",
                    ));
                }
                Ok(())
            }
        }
    };
}

docker_client_test_suite!(bollard, bollard_client);
docker_client_test_suite!(
    rs_docker,
    rs_docker_client,
    skip = "rs-docker unmaintained and not actively maintained"
);
docker_client_test_suite!(docker_wrapper, docker_wrapper_client);
