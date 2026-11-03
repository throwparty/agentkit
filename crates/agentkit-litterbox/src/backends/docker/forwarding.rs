//! Docker's host port allocation and forwarding.

use std::collections::HashMap;
use std::net::TcpListener;

use super::compute::PortBindingSpec;
use crate::sandbox::contract::Error;
use crate::sandbox::contract::{Forward, RequestedPort};

pub(crate) fn env_var_for_slug(slug: &str) -> String {
    format!(
        "LITTERBOX_FWD_PORT_{}",
        slug.replace('-', "_").to_ascii_uppercase()
    )
}

/// Allocate an ephemeral host port on a specific bind address.
///
/// The kernel chooses the port, so it is neither predictable to another local
/// process nor confined to a range Litterbox guesses at. The probe is released
/// immediately; a daemon bind conflict is retried by the backend.
fn allocate_host_port_on(bind: &str) -> Result<u16, Error> {
    let listener = TcpListener::bind((bind, 0)).map_err(|error| {
        Error::failed(format!("could not reserve a host port on {bind}: {error}"))
    })?;
    let port = listener
        .local_addr()
        .map_err(|error| Error::failed(format!("could not read the reserved port: {error}")))?
        .port();
    drop(listener);
    Ok(port)
}

/// A port name reduced to the shape an environment variable can carry.
fn env_slug(name: &str) -> Result<String, Error> {
    let slug = name.trim().to_ascii_lowercase().replace(' ', "-");
    let valid = !slug.is_empty()
        && slug.len() <= 63
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        return Err(Error::failed(format!(
            "'{name}' is not a valid port name; use lowercase letters, digits and hyphens"
        )));
    }
    Ok(slug)
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
    Error,
> {
    let mut env = Vec::new();
    let mut port_bindings: HashMap<String, Vec<PortBindingSpec>> = HashMap::new();
    let mut forwards = Vec::new();

    for port in ports {
        let slug = env_slug(&port.name)?;
        let env_key = env_var_for_slug(&slug);
        let host_port = allocate_host_port_on(bind)?;
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
