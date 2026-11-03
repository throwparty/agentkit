//! Docker's host port allocation and forwarding.

use std::collections::HashMap;
use std::net::TcpListener;

use super::compute::PortBindingSpec;
use crate::sandbox::contract::Error;
use crate::sandbox::contract::{Forward, RequestedPort, forwarded_port_env_var};

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

/// Build the port bindings and forwards a create request asks for.
pub(crate) async fn forwarded_ports(
    ports: &[RequestedPort],
    bind: &str,
) -> Result<(HashMap<String, Vec<PortBindingSpec>>, Vec<Forward>), Error> {
    let mut port_bindings: HashMap<String, Vec<PortBindingSpec>> = HashMap::new();
    let mut forwards = Vec::new();

    for port in ports {
        let env_key = forwarded_port_env_var(&port.name)
            .map_err(|message| Error::failed(message.to_string()))?;
        let host_port = allocate_host_port_on(bind)?;
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

    Ok((port_bindings, forwards))
}
