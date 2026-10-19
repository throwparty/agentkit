//! The ACP command surface: available-commands advertisement, the /mcp and /editor commands, server connection, and the first-turn MCP status notice.

use super::client_tools;
use super::negotiation::ConnectionNegotiation;
use crate::mcp::{McpPool, ServerStatus};
use crate::state::TackleState;
use agent_client_protocol::schema::v1::{
    AvailableCommandsUpdate, Notice, NoticeSeverity, SessionUpdate,
};

/// The `available_commands_update` payload for a session: the registry
/// view over the loaded definitions — the `!` prefix command plus every
/// user-invokable prompt, with parameter hints from frontmatter. The
/// registry carries the pool's MCP tools here; individual tools are
/// never advertised as commands (only reached through `/!`).
///
/// The tool listing is per-connection state, not configuration, so it is
/// read live: a server disabled by `/mcp disable` drops out of the
/// listing for the rest of the connection.
pub(super) async fn available_commands_update(
    state: &TackleState,
    negotiation: &ConnectionNegotiation,
) -> SessionUpdate {
    let pool = negotiation.mcp_pool();
    let pool = pool.lock().await;
    let mcp_tools = pool.list_tools().await;
    drop(pool);

    let client_tools =
        client_tools::catalog(negotiation.client_caps(), negotiation.editor_toggles());
    let registry = match crate::invokables::Registry::build_with_client_tools(
        &state.definitions(),
        &mcp_tools,
        &client_tools,
    ) {
        Ok(registry) => registry,
        Err(err) => {
            // Namespaced keys over BTreeMap definitions cannot collide;
            // degrade to the bare registry rather than failing the
            // session lifecycle on a configuration error.
            tracing::warn!("registry build failed for available_commands_update: {err}");
            crate::invokables::Registry::default()
        }
    };
    SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(
        registry.available_commands(),
    ))
}

/// Maps an ACP client-forwarded MCP server onto the pool's spawn
/// configuration. SSE and ACP transports are not supported (tackle
/// advertises neither), so those entries are skipped; HTTP headers are
/// not modelled by the pool configuration and are dropped.
pub(super) fn forwarded_mcp_server(
    server: &agent_client_protocol::schema::v1::McpServer,
) -> Option<(String, crate::config::McpServerConfig)> {
    use agent_client_protocol::schema::v1::McpServer;
    match server {
        McpServer::Stdio(stdio) => Some((
            stdio.name.clone(),
            crate::config::McpServerConfig::Stdio {
                command: stdio.command.to_string_lossy().into_owned(),
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
            crate::config::McpServerConfig::Http {
                url: http.url.clone(),
            },
        )),
        _ => None,
    }
}

/// Registers and connects the client-forwarded MCP servers for a
/// session, on the connection's own pool. Failures surface as pool
/// statuses; a session is never failed by a forwarded server.
pub(super) async fn connect_forwarded_servers(
    negotiation: &ConnectionNegotiation,
    servers: &[agent_client_protocol::schema::v1::McpServer],
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

/// Builds a connection's MCP pool: every configured server is registered
/// and connected concurrently, so a dead server is a status rather than a
/// startup failure.
pub(crate) async fn connect_config_servers(state: &TackleState) -> McpPool {
    let helper = crate::credentials::credential_helper_name(&state.config().config);
    let new_pool = || McpPool::new().with_credential_helper(helper.clone());
    let mut pool = new_pool();
    for (server_name, server_config) in &state.config().config.mcp_servers {
        pool.register(server_name, server_config.clone());
    }
    let connecting = state
        .config()
        .config
        .mcp_servers
        .iter()
        .map(|(server_name, server_config)| {
            let server_name = server_name.clone();
            let server_config = server_config.clone();
            let mut one = new_pool();
            one.register(&server_name, server_config.clone());
            async move {
                one.connect(&server_name, &server_config).await;
                one
            }
        })
        .collect::<Vec<_>>();
    for connected in futures::future::join_all(connecting).await {
        pool.absorb(connected);
    }
    pool
}

/// The first-turn MCP report: a warning notice naming enabled
/// servers that failed to connect and config servers shadowed by a
/// client-declared server of the same name. `None` when nothing is
/// wrong, so a clean session stays silent.
pub(super) async fn mcp_status_notice(negotiation: &ConnectionNegotiation) -> Option<Notice> {
    let pool = negotiation.mcp_pool();
    let pool = pool.lock().await;
    let mut problems: Vec<String> = pool
        .shadowed_servers()
        .into_iter()
        .map(|name| format!("{name} (shadowed by a client-declared server)"))
        .collect();
    for (name, (status, enabled)) in pool.statuses() {
        if !enabled {
            continue;
        }
        if let ServerStatus::Failed { reason } = status {
            problems.push(format!("{name} ({reason})"));
        }
    }
    if problems.is_empty() {
        return None;
    }
    Some(
        Notice::new(
            NoticeSeverity::Warning,
            format!("MCP servers unavailable: {}", problems.join(", ")),
        )
        .description(Some(
            "Run /mcp status for the full picture, or /mcp enable <name> to retry.".to_owned(),
        )),
    )
}

/// One server's line in the `/mcp status` report. A disabled server
/// reads as disabled and carries no connection state: disabling closes
/// the connection, so a status alongside it would be noise.
pub(super) fn mcp_status_line(name: &str, status: &ServerStatus, enabled: bool) -> String {
    if !enabled {
        return format!("{name}: disabled");
    }
    match status {
        ServerStatus::Connected => format!("{name}: connected"),
        ServerStatus::Failed { reason } => format!("{name}: failed ({reason})"),
    }
}

/// Handles a `/mcp` command, returning the text to answer the turn with,
/// or `None` for a subcommand that is not ours so the caller can fall
/// through to normal processing.
///
/// The user is the authority on availability, so a disabled server is
/// one keystroke away from being off for the rest of the connection and
/// one from being back. Enabling reconnects from the config held at
/// registration; a failure to reconnect answers with the reason rather
/// than leaving the user guessing.
pub(super) async fn mcp_command(
    negotiation: &ConnectionNegotiation,
    first_text: &str,
) -> Option<String> {
    let mut parts = first_text.split_whitespace();
    parts.next(); // the `/mcp` command itself
    let (subcommand, argument) = match parts.next() {
        None => return Some("Usage: /mcp <enable|disable|status> [server-name]".to_owned()),
        Some(subcommand) => (subcommand, parts.next()),
    };

    let pool = negotiation.mcp_pool();
    let mut pool = pool.lock().await;
    match subcommand {
        "enable" => Some(match argument {
            None => "Usage: /mcp enable <server-name>".to_owned(),
            Some(name) => match pool.enable_server(name).await {
                Ok(()) if pool.is_connected(name) => format!("Enabled MCP server: {name}"),
                Ok(()) => format!("Enabled MCP server: {name} (not connected)"),
                Err(err) => format!("Failed to enable MCP server {name}: {err}"),
            },
        }),
        "disable" => Some(match argument {
            None => "Usage: /mcp disable <server-name>".to_owned(),
            Some(name) => match pool.disable_server(name) {
                Ok(()) => format!("Disabled MCP server: {name}"),
                Err(err) => format!("Failed to disable MCP server {name}: {err}"),
            },
        }),
        "status" => Some(match argument {
            None => {
                let statuses = pool.statuses();
                if statuses.is_empty() {
                    "No MCP servers configured".to_owned()
                } else {
                    statuses
                        .iter()
                        .map(|(name, (status, enabled))| mcp_status_line(name, status, *enabled))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            }
            Some(name) => match pool.statuses().get(name) {
                Some((status, enabled)) => mcp_status_line(name, status, *enabled),
                None => format!("MCP server not found: {name}"),
            },
        }),
        _ => None,
    }
}

/// Handles `/editor <fs|terminal> <true|false>`: the runtime toggle for
/// the client-capability categories. The effective surface also requires
/// the client to have advertised the capability; a toggle takes effect
/// the following turn (the registry re-reads it per turn). Anything not
/// matching the shape yields usage, never a silent no-op.
pub(super) fn editor_command(
    negotiation: &ConnectionNegotiation,
    first_text: &str,
) -> Option<String> {
    const USAGE: &str = "Usage: /editor <fs|terminal> <true|false>";
    let mut parts = first_text.split_whitespace();
    parts.next(); // the `/editor` command itself
    let (kind, value) = match (parts.next(), parts.next()) {
        (Some(kind), Some(value)) => (kind, value),
        _ => return Some(USAGE.to_owned()),
    };
    let enabled = match value {
        "true" => true,
        "false" => false,
        _ => return Some(USAGE.to_owned()),
    };
    let mut toggles = negotiation.editor.lock().expect("negotiation poisoned");
    match kind {
        "fs" => toggles.fs = enabled,
        "terminal" => toggles.terminal = enabled,
        _ => return Some(USAGE.to_owned()),
    }
    Some(format!(
        "Editor {kind} tools: {}",
        if enabled { "enabled" } else { "disabled" }
    ))
}
