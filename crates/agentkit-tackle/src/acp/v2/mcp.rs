//! Client-forwarded MCP servers: v2 session lifecycle requests register and
//! connect them through the shared pool.

use super::negotiation::ConnectionNegotiation;
use crate::config::McpServerConfig;
use agent_client_protocol::schema::v2::McpServer;

/// Maps a v2 client-forwarded MCP server onto the pool's spawn configuration.
/// SSE and ACP transports are not advertised, so those entries are skipped;
/// HTTP headers are not modelled by the pool configuration and are dropped.
fn forwarded_mcp_server(server: &McpServer) -> Option<(String, McpServerConfig)> {
    match server {
        McpServer::Stdio(stdio) => Some((
            stdio.name.clone(),
            McpServerConfig::Stdio {
                command: stdio.command.0.to_string_lossy().into_owned(),
                args: stdio.args.clone(),
                env: stdio
                    .env
                    .iter()
                    .map(|variable| (variable.name.clone(), variable.value.clone()))
                    .collect(),
            },
        )),
        McpServer::Http(http) => Some((
            http.name.clone(),
            McpServerConfig::Http {
                url: http.url.clone(),
            },
        )),
        _ => None,
    }
}

/// Registers and connects the servers a client forwarded on a session request.
pub(super) async fn connect_forwarded_servers(
    negotiation: &ConnectionNegotiation,
    servers: &[McpServer],
) {
    let forwarded: Vec<_> = servers.iter().filter_map(forwarded_mcp_server).collect();
    if forwarded.is_empty() {
        return;
    }
    negotiation
        .mcp_pool()
        .lock()
        .await
        .add_forwarded(forwarded)
        .await;
}
