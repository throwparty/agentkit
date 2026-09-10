use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{ContainerSpec, ContainerSummary, DockerClient, DockerResult, ExecOutput};
use rs_docker::Docker;
use tokio::task;
use urlencoding::encode;

use hyper::client::HttpConnector;
use hyper::{Body, Client, Method, Request, Uri};
use hyperlocal::UnixConnector;
use serde_json::Value;
use tar::{Archive, Builder};

pub struct RsDockerClient {
    docker: Arc<Mutex<Docker>>,
    http: DockerHttp,
}

impl RsDockerClient {
    pub fn connect_with_env() -> DockerResult<Self> {
        let addr = std::env::var("DOCKER_HOST")
            .unwrap_or_else(|_| "unix:///var/run/docker.sock".to_string());
        let docker = Docker::connect(&addr)?;
        let http = DockerHttp::new(&addr)?;
        Ok(Self {
            docker: Arc::new(Mutex::new(docker)),
            http,
        })
    }

    fn split_image(image: &str) -> (String, String) {
        let mut parts = image.splitn(2, ':');
        let name = parts.next().unwrap_or(image).to_string();
        let tag = parts.next().unwrap_or("latest").to_string();
        (name, tag)
    }

    fn default_container_name() -> String {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        format!("rs_docker_container_{}", millis)
    }

    async fn with_docker<T, F>(&self, op: F) -> DockerResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Docker) -> std::io::Result<T> + Send + 'static,
    {
        let docker = self.docker.clone();
        let result = task::spawn_blocking(move || {
            let mut docker = docker.lock().unwrap();
            op(&mut docker)
        })
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        result.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }
}

enum Protocol {
    Unix,
    Tcp,
}

struct DockerHttp {
    protocol: Protocol,
    path: String,
    hyperlocal_client: Option<Client<UnixConnector, Body>>,
    hyper_client: Option<Client<HttpConnector, Body>>,
}

impl DockerHttp {
    fn new(addr: &str) -> DockerResult<Self> {
        let components: Vec<&str> = addr.split("://").collect();
        if components.len() != 2 {
            return Err("invalid DOCKER_HOST format".into());
        }

        let protocol = match components[0] {
            "unix" => Protocol::Unix,
            "tcp" => Protocol::Tcp,
            _ => return Err("unsupported DOCKER_HOST protocol".into()),
        };

        let path = components[1].to_string();

        let hyperlocal_client = match protocol {
            Protocol::Unix => Some(Client::builder().build(UnixConnector)),
            Protocol::Tcp => None,
        };

        let hyper_client = match protocol {
            Protocol::Tcp => Some(Client::new()),
            Protocol::Unix => None,
        };

        Ok(Self {
            protocol,
            path,
            hyperlocal_client,
            hyper_client,
        })
    }

    async fn request_bytes(
        &self,
        method: Method,
        url: &str,
        body: Vec<u8>,
        content_type: &str,
    ) -> DockerResult<Vec<u8>> {
        let uri = match self.protocol {
            Protocol::Unix => hyperlocal::Uri::new(self.path.clone(), url).into(),
            Protocol::Tcp => format!("http://{}{}", self.path, url)
                .parse::<Uri>()
                .map_err(|e| e.to_string())?,
        };

        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("Content-Type", content_type)
            .header("Accept", "application/json")
            .body(Body::from(body))
            .map_err(|e| e.to_string())?;

        let response = match self.protocol {
            Protocol::Unix => {
                self.hyperlocal_client
                    .as_ref()
                    .unwrap()
                    .request(request)
                    .await
            }
            Protocol::Tcp => self.hyper_client.as_ref().unwrap().request(request).await,
        }
        .map_err(|e| e.to_string())?;

        let status = response.status();
        let bytes = hyper::body::to_bytes(response.into_body())
            .await
            .map_err(|e| e.to_string())?;
        if !status.is_success() {
            let body_text = String::from_utf8_lossy(&bytes);
            return Err(format!("docker api error {}: {}", status, body_text.trim()).into());
        }

        Ok(bytes.to_vec())
    }
}

#[async_trait::async_trait]
impl DockerClient for RsDockerClient {
    async fn pull_image(&self, image: &str) -> DockerResult<()> {
        let (name, tag) = Self::split_image(image);
        let url = format!(
            "/images/create?fromImage={}&tag={}",
            encode(&name),
            encode(&tag)
        );
        self.http
            .request_bytes(Method::POST, &url, Vec::new(), "application/json")
            .await?;
        Ok(())
    }

    async fn create_container(&self, _spec: ContainerSpec) -> DockerResult<String> {
        let name = _spec.name.unwrap_or_else(Self::default_container_name);
        let body = serde_json::json!({
            "Image": _spec.image,
            "Cmd": if _spec.cmd.is_empty() { Value::Null } else { Value::from(_spec.cmd) }
        });
        let url = format!("/containers/create?name={}", encode(&name));
        let response = self
            .http
            .request_bytes(
                Method::POST,
                &url,
                serde_json::to_vec(&body)?,
                "application/json",
            )
            .await?;
        let value: Value = serde_json::from_slice(&response)?;
        let id = value
            .get("Id")
            .and_then(|id| id.as_str())
            .ok_or_else(|| "docker create response missing Id")?;
        Ok(id.to_string())
    }

    async fn start_container(&self, _container_id: &str) -> DockerResult<()> {
        let id = _container_id.to_string();
        self.with_docker(move |docker| {
            docker.start_container(&id)?;
            Ok(())
        })
        .await
    }

    async fn stop_container(&self, _container_id: &str) -> DockerResult<()> {
        let id = _container_id.to_string();
        self.with_docker(move |docker| {
            docker.stop_container(&id)?;
            Ok(())
        })
        .await
    }

    async fn remove_container(&self, _container_id: &str, _force: bool) -> DockerResult<()> {
        if _force {
            let _ = self.stop_container(_container_id).await;
        }
        let id = _container_id.to_string();
        self.with_docker(move |docker| {
            docker.delete_container(&id)?;
            Ok(())
        })
        .await
    }

    async fn list_containers(&self, all: bool) -> DockerResult<Vec<ContainerSummary>> {
        self.with_docker(move |docker| docker.get_containers(all))
            .await
            .map(|containers| {
                containers
                    .into_iter()
                    .map(|container| ContainerSummary {
                        id: container.Id,
                        names: container.Names,
                        status: Some(container.Status),
                    })
                    .collect()
            })
    }

    async fn exec(&self, _container_id: &str, _cmd: Vec<String>) -> DockerResult<ExecOutput> {
        let exec_body = serde_json::json!({
            "AttachStdout": true,
            "AttachStderr": true,
            "Tty": true,
            "Cmd": _cmd,
        });
        let exec_url = format!("/containers/{}/exec", _container_id);
        let exec_response = self
            .http
            .request_bytes(
                Method::POST,
                &exec_url,
                serde_json::to_vec(&exec_body)?,
                "application/json",
            )
            .await?;
        let exec_value: Value = serde_json::from_slice(&exec_response)?;
        let exec_id = exec_value
            .get("Id")
            .and_then(|id| id.as_str())
            .ok_or_else(|| "docker exec response missing Id")?;

        let start_body = serde_json::json!({
            "Detach": false,
            "Tty": true
        });
        let start_url = format!("/exec/{}/start", exec_id);
        let output = self
            .http
            .request_bytes(
                Method::POST,
                &start_url,
                serde_json::to_vec(&start_body)?,
                "application/json",
            )
            .await?;

        Ok(ExecOutput {
            stdout: output,
            stderr: Vec::new(),
        })
    }

    async fn copy_file_into(
        &self,
        _container_id: &str,
        _host_path: &Path,
        _container_path: &Path,
    ) -> DockerResult<()> {
        let filename = _container_path
            .file_name()
            .ok_or_else(|| "container path must include file name")?;
        let parent = _container_path.parent().unwrap_or_else(|| Path::new("/"));

        let mut tar_builder = Builder::new(Vec::new());
        let mut file_to_tar = std::fs::File::open(_host_path)?;
        tar_builder.append_file(Path::new(filename), &mut file_to_tar)?;
        let tar_data = tar_builder.into_inner()?;

        let url = format!(
            "/containers/{}/archive?path={}",
            _container_id,
            encode(parent.to_string_lossy().as_ref())
        );
        self.http
            .request_bytes(Method::PUT, &url, tar_data, "application/x-tar")
            .await?;
        Ok(())
    }

    async fn copy_file_out(
        &self,
        _container_id: &str,
        _container_path: &Path,
        _host_path: &Path,
    ) -> DockerResult<()> {
        let url = format!(
            "/containers/{}/archive?path={}",
            _container_id,
            encode(_container_path.to_string_lossy().as_ref())
        );
        let tar_bytes = self
            .http
            .request_bytes(Method::GET, &url, Vec::new(), "application/json")
            .await?;

        if let Some(parent) = _host_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut archive = Archive::new(std::io::Cursor::new(tar_bytes));
        let mut entries = archive.entries()?;
        let mut entry = entries
            .next()
            .ok_or_else(|| "container archive was empty")??;
        let mut output = std::fs::File::create(_host_path)?;
        std::io::copy(&mut entry, &mut output)?;
        Ok(())
    }
}
