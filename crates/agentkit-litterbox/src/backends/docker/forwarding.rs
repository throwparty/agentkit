//! Docker's host port allocation and forwarding.

use std::collections::HashMap;
use std::net::TcpListener;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::time::sleep;

use super::compute::PortBindingSpec;
use crate::domain::{ForwardedPortMapping, SandboxConfig, SandboxError, slugify_name};
use crate::sandbox::contract::{Forward, RequestedPort};

pub(crate) const DEFAULT_PORT_RANGE_START: u16 = 3000;
pub(crate) const DEFAULT_PORT_RANGE_END: u16 = 8000;
pub(crate) const PORT_ALLOC_BACKOFF_MS: u64 = 25;
pub(crate) const PORT_ALLOC_MAX_RETRIES: usize = 32;

pub(crate) async fn build_forwarded_ports(
    config: &SandboxConfig,
) -> Result<
    (
        Vec<String>,
        HashMap<String, Vec<PortBindingSpec>>,
        Vec<ForwardedPortMapping>,
    ),
    SandboxError,
> {
    if config.forwarded_ports.is_empty() {
        return Ok((Vec::new(), HashMap::new(), Vec::new()));
    }

    let mut env = Vec::new();
    let mut port_bindings: HashMap<String, Vec<PortBindingSpec>> = HashMap::new();
    let mut forwarded = Vec::new();

    for port in &config.forwarded_ports {
        let slug = slugify_name(&port.name)?;
        let env_key = env_var_for_slug(&slug);
        let host_port =
            allocate_host_port(DEFAULT_PORT_RANGE_START, DEFAULT_PORT_RANGE_END).await?;
        env.push(format!("{env_key}={host_port}"));
        port_bindings.insert(
            format!("{}/tcp", port.target),
            vec![PortBindingSpec {
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

pub(crate) fn env_var_for_slug(slug: &str) -> String {
    format!(
        "LITTERBOX_FWD_PORT_{}",
        slug.replace('-', "_").to_ascii_uppercase()
    )
}

pub(crate) async fn allocate_host_port(
    range_start: u16,
    range_end: u16,
) -> Result<u16, SandboxError> {
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

/// Allocate a host port on a specific bind address.
pub(crate) async fn allocate_host_port_on(
    bind: &str,
    range_start: u16,
    range_end: u16,
) -> Result<u16, SandboxError> {
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
        if TcpListener::bind((bind, candidate)).is_ok() {
            return Ok(candidate);
        }
        sleep(Duration::from_millis(PORT_ALLOC_BACKOFF_MS)).await;
    }

    Err(SandboxError::Config(format!(
        "No available host ports in range {range_start}-{range_end}"
    )))
}

/// Build the environment, port bindings and forwards a create request asks for.
pub(crate) async fn forwarded_ports(
    ports: &[RequestedPort],
    bind: &str,
) -> Result<
    (
        Vec<String>,
        HashMap<String, Vec<PortBindingSpec>>,
        Vec<Forward>,
    ),
    SandboxError,
> {
    let mut env = Vec::new();
    let mut port_bindings: HashMap<String, Vec<PortBindingSpec>> = HashMap::new();
    let mut forwards = Vec::new();

    for port in ports {
        let slug = slugify_name(&port.name)?;
        let env_key = env_var_for_slug(&slug);
        let host_port =
            allocate_host_port_on(bind, DEFAULT_PORT_RANGE_START, DEFAULT_PORT_RANGE_END).await?;
        env.push(format!("{env_key}={host_port}"));
        port_bindings.insert(
            format!("{}/tcp", port.target),
            vec![PortBindingSpec {
                host_ip: Some(bind.to_string()),
                host_port: Some(host_port.to_string()),
            }],
        );
        forwards.push(Forward {
            name: port.name.clone(),
            target: port.target,
            host_port,
            bind: bind.to_string(),
            env_var: env_key,
        });
    }

    Ok((env, port_bindings, forwards))
}
