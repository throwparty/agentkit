//! Docker as a sandboxing mechanism.
//!
//! Only wiring lives here: the backend, its API client, its section and its
//! naming and forwarding helpers are siblings.

pub mod backend;
pub mod compute;
pub mod config;
pub mod construct;
pub(crate) mod error;
pub(crate) mod forwarding;
pub(crate) mod naming;

pub use backend::{BACKEND, DockerSettings};

/// Whether the Docker endpoint the client would use exists, for tests that
/// construct a backend but do not exercise the daemon.
#[cfg(test)]
pub(crate) fn socket_available() -> bool {
    let host = std::env::var("DOCKER_HOST")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let path = match host {
        Some(host) => match host.strip_prefix("unix://") {
            Some(path) => path.to_string(),
            None if host.starts_with('/') => host,
            None => return false,
        },
        None => "/var/run/docker.sock".to_string(),
    };
    std::path::Path::new(&path).exists()
}
