//! The v2 command surface: the registry view over the loaded definitions and
//! MCP tools, rendered with v2's `type`-discriminated command input.

use super::negotiation::ConnectionNegotiation;
use crate::state::TackleState;
use agent_client_protocol::schema::v1::AvailableCommandInput;
use agent_client_protocol::schema::v2::{
    AvailableCommand, AvailableCommandInput as WireInput, TextCommandInput,
};

/// The `available_commands` list for a session: the `!` prefix command, each
/// MCP tool under its own `!`-prefixed name, and every user-invokable prompt,
/// mapped onto v2's text input shape.
pub(super) async fn available_commands(
    state: &TackleState,
    negotiation: &ConnectionNegotiation,
) -> Vec<AvailableCommand> {
    let pool = negotiation.mcp_pool();
    let pool = pool.lock().await;
    let mcp_tools = pool.list_tools().await;
    drop(pool);

    let registry = match crate::invokables::Registry::build_with_client_tools(
        &state.definitions(),
        &mcp_tools,
        &[],
    ) {
        Ok(registry) => registry,
        Err(err) => {
            tracing::warn!("registry build failed for v2 available commands: {err}");
            crate::invokables::Registry::default()
        }
    };
    registry
        .available_commands()
        .into_iter()
        // `/editor` toggles the v1 client-tool surface; v2 has none.
        .filter(|command| command.name != "editor")
        .map(|command| {
            let hint = match command.input.as_ref() {
                Some(AvailableCommandInput::Unstructured(input)) => {
                    if command.name == "!" {
                        // v2 has no client tools, so the v1 hint would be a lie.
                        "mcp.<server>.<tool> {json arguments}".to_owned()
                    } else {
                        input.hint.clone()
                    }
                }
                _ => String::new(),
            };
            let mut out = AvailableCommand::new(command.name, command.description);
            if !hint.is_empty() {
                out = out.input(WireInput::Text(TextCommandInput::new(hint)));
            }
            out
        })
        .collect()
}

/// One `/mcp status` line.
pub(super) fn mcp_status_line(name: &str, status: &crate::mcp::ServerStatus, enabled: bool) -> String {
    if !enabled {
        return format!("{name}: disabled");
    }
    match status {
        crate::mcp::ServerStatus::Connected => format!("{name}: connected"),
        crate::mcp::ServerStatus::Failed { reason } => format!("{name}: failed ({reason})"),
    }
}

/// Handles `/mcp <enable|disable|status> [server]`, returning the reply or
/// `None` for a subcommand that is not ours so the caller can fall through.
pub(super) async fn mcp_command(
    negotiation: &ConnectionNegotiation,
    first_text: &str,
) -> Option<String> {
    let mut parts = first_text.split_whitespace();
    parts.next();
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
