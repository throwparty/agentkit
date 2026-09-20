//! The MCP client pool: connections to configured and client-provided
//! servers, tools namespaced `mcp.<server>.<tool>`, per-server status.
//!
//! Connection is asynchronous with a per-server timeout, so session
//! creation never blocks; the status of every server (connected/failed)
//! is reported at the first turn. Nothing about which servers exist is
//! hardcoded.

use crate::config::McpServerConfig;
use rmcp::model::CallToolRequestParams;
use rmcp::service::{RoleClient, RunningService};
use std::collections::BTreeMap;
use std::time::Duration;

/// Tool results are truncated at this size; full content is what the
/// server sent (the truncation is a context/cost guard).
pub const TOOL_RESULT_LIMIT: usize = 16 * 1024;

/// Connection timeout per server: session creation must never block.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

async fn connect_timeout<T>(future: impl std::future::Future<Output = T>) -> Result<T, String> {
    tokio::time::timeout(CONNECT_TIMEOUT, future)
        .await
        .map_err(|_| "connection exceeded the 10s per-server timeout".to_owned())
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerStatus {
    Connected,
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamespacedTool {
    /// `mcp.<server>.<tool>`.
    pub name: String,
    pub description: String,
    /// The tool's input schema, verbatim from the server.
    pub schema: serde_json::Value,
}

/// A tool result: the content blocks (serialised), the error flag, and
/// the truncation marker.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub content_json: String,
    pub is_error: bool,
    pub truncated: bool,
}

pub struct McpPool {
    connections: BTreeMap<String, Connection>,
    statuses: BTreeMap<String, ServerStatus>,
}

struct Connection {
    peer: RunningService<RoleClient, ()>,
}

impl Default for McpPool {
    fn default() -> Self {
        Self::new()
    }
}

impl McpPool {
    /// An empty pool.
    pub fn new() -> Self {
        Self {
            connections: BTreeMap::new(),
            statuses: BTreeMap::new(),
        }
    }

    /// Statuses for the first-turn report: every declared server with its
    /// connected/failed state.
    pub fn statuses(&self) -> &BTreeMap<String, ServerStatus> {
        &self.statuses
    }

    /// Connects one configured server (stdio argv or HTTP URL), recording
    /// the outcome as status. Never panics; failures are statuses.
    pub async fn connect(&mut self, name: &str, config: &McpServerConfig) {
        let result: Result<RunningService<RoleClient, ()>, String> = async {
            match config {
                McpServerConfig::Stdio { command, args, env } => {
                    let mut command = tokio::process::Command::new(command);
                    command.args(args).envs(env);
                    let transport = rmcp::transport::TokioChildProcess::new(command)
                        .map_err(|err| err.to_string())?;
                    match connect_timeout(rmcp::service::serve_client((), transport)).await {
                        Ok(initialised) => initialised.map_err(|err| err.to_string()),
                        Err(reason) => Err(reason),
                    }
                }
                McpServerConfig::Http { url } => {
                    let transport =
                        rmcp::transport::StreamableHttpClientTransport::from_uri(url.clone());
                    match connect_timeout(rmcp::service::serve_client((), transport)).await {
                        Ok(initialised) => initialised.map_err(|err| err.to_string()),
                        Err(reason) => Err(reason),
                    }
                }
            }
        }
        .await;
        match result {
            Ok(peer) => {
                self.connections
                    .insert(name.to_owned(), Connection { peer });
                self.statuses
                    .insert(name.to_owned(), ServerStatus::Connected);
            }
            Err(reason) => {
                self.statuses
                    .insert(name.to_owned(), ServerStatus::Failed { reason });
            }
        }
    }

    /// Whether a server is connected (its tools are callable).
    pub fn is_connected(&self, name: &str) -> bool {
        self.connections.contains_key(name)
    }

    /// All connected servers' tools, namespaced `mcp.<server>.<tool>`.
    pub async fn list_tools(&self) -> Vec<NamespacedTool> {
        let mut tools = Vec::new();
        for (server, connection) in &self.connections {
            let peer = connection.peer.peer();
            let listed = match peer.list_tools(None).await {
                Ok(listed) => listed,
                Err(_) => continue, // a dead server yields no tools
            };
            for tool in listed.tools {
                tools.push(NamespacedTool {
                    name: format!("mcp.{server}.{}", tool.name),
                    description: tool
                        .description
                        .as_ref()
                        .map(|description| description.to_string())
                        .unwrap_or_default(),
                    schema: serde_json::Value::Object((*tool.input_schema.clone()).clone()),
                });
            }
        }
        tools
    }

    /// Calls a tool on its server; the result content is serialised (with
    /// truncation at the fixed internal limit) and the error flag carried.
    pub async fn call_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolResult, crate::mcp::McpPoolError> {
        let connection = self
            .connections
            .get(server)
            .ok_or_else(|| crate::mcp::McpPoolError::ServerNotConnected(server.to_owned()))?;
        let arguments = match arguments {
            serde_json::Value::Null => None,
            serde_json::Value::Object(map) => Some(map),
            other => Some(serde_json::from_value(other).expect("object map")),
        };
        let mut params = CallToolRequestParams::new(tool.to_owned());
        params.arguments = arguments;
        let result = connection
            .peer
            .call_tool(params)
            .await
            .map_err(|err| crate::mcp::McpPoolError::Call(err.to_string()))?;

        let mut content_json =
            serde_json::to_string(&result.content).unwrap_or_else(|_| "[]".to_owned());
        let mut truncated = false;
        if content_json.len() > TOOL_RESULT_LIMIT {
            content_json.truncate(TOOL_RESULT_LIMIT);
            truncated = true;
        }
        Ok(ToolResult {
            content_json,
            is_error: result.is_error.unwrap_or(false),
            truncated,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum McpPoolError {
    #[error("MCP server `{0}` is not connected")]
    ServerNotConnected(String),
    #[error("tool call failed: {0}")]
    Call(String),
}
