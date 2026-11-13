//! Client-forwarded MCP servers: v2 session lifecycle requests register and
//! connect them through the shared pool.

use super::negotiation::ConnectionNegotiation;
use crate::config::McpServerConfig;
use agent_client_protocol::v2::schema::McpServer;
use agent_client_protocol::Error;

/// Maps a v2 client-forwarded MCP server onto the pool's spawn configuration.
/// SSE and ACP transports are not advertised, so those entries are skipped;
/// HTTP headers are carried through to every request.
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
                    // A client-supplied env must never resolve a stored
                    // credential marker; only user/project config may.
                    .filter(|variable| !variable.value.contains("{cred:"))
                    .map(|variable| (variable.name.clone(), variable.value.clone()))
                    .collect(),
            },
        )),
        McpServer::Http(http) => Some((
            http.name.clone(),
            McpServerConfig::Http {
                url: http.url.clone(),
                headers: http
                    .headers
                    .iter()
                    .map(|header| (header.name.clone(), header.value.clone()))
                    .collect(),
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
    let mut forwarded = Vec::new();
    for server in servers {
        match forwarded_mcp_server(server) {
            Some(entry) => forwarded.push(entry),
            None => tracing::warn!(
                version = "v2",
                "forwarded MCP server uses an unsupported transport; skipped"
            ),
        }
    }
    if forwarded.is_empty() {
        return;
    }
    crate::mcp::connect_forwarded(negotiation.mcp_pool(), forwarded).await;
    negotiation.invalidate_commands().await;
}

/// Checks the servers a client forwarded before a session is created or
/// resumed: `-32602` for a transport this agent does not serve (only stdio and
/// HTTP are advertised) or for two servers sharing a name, which the pool
/// could not tell apart. A request is never half-honoured: either every
/// forwarded server is usable, or the session-scoped request is refused.
pub(super) fn validate_forwarded_servers(servers: &[McpServer]) -> Result<(), Error> {
    let mut names = std::collections::BTreeSet::new();
    for server in servers {
        let Some((name, _)) = forwarded_mcp_server(server) else {
            return Err(Error::invalid_params().data(serde_json::json!({
                "error": "unsupported MCP server transport",
            })));
        };
        if !names.insert(name.clone()) {
            return Err(Error::invalid_params().data(serde_json::json!({
                "error": "duplicate MCP server name",
                "name": name,
            })));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::v2::schema::{
        EnvVariable, HttpHeader, McpServerHttp, McpServerStdio,
    };

    #[test]
    fn forwarded_http_headers_are_carried() {
        let server = McpServer::Http(
            McpServerHttp::new("x", "https://example.com/mcp")
                .headers(vec![HttpHeader::new("Authorization", "Bearer token")]),
        );
        let (_, config) = forwarded_mcp_server(&server).expect("http server");
        let McpServerConfig::Http { headers, .. } = config else {
            panic!("expected an http server");
        };
        assert_eq!(
            headers.get("Authorization"),
            Some(&"Bearer token".to_owned())
        );
    }

    #[test]
    fn forwarded_credential_markers_are_dropped() {
        let server = McpServer::Stdio(
            McpServerStdio::new("x", std::path::PathBuf::from("/bin/true")).env(vec![
                EnvVariable::new("A", "{cred:secret}"),
                EnvVariable::new("B", "plain"),
            ]),
        );
        let (_, config) = forwarded_mcp_server(&server).expect("stdio server");
        let McpServerConfig::Stdio { env, .. } = config else {
            panic!("expected a stdio server");
        };
        assert!(!env.contains_key("A"));
        assert_eq!(env.get("B"), Some(&"plain".to_owned()));
    }
}
