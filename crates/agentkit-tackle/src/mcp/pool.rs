//! The MCP client pool: connections, tool namespacing, per-server status, and elicitation forwarding.

use super::spawn::configure_stdio_spawn;
use super::stderr::{relay_stderr, StderrRateCap, STDERR_RELAY_MAX_LINES, STDERR_RELAY_WINDOW};
use crate::config::McpServerConfig;
use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientConfig, ElicitRequestParams, ElicitResult,
    ElicitationAction, Implementation,
};
use rmcp::service::{RequestContext, RoleClient, RunningService};
use rmcp::ClientHandler;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

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
/// the truncation marker with the original size.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub content_json: String,
    pub is_error: bool,
    pub truncated: bool,
    /// The serialised size before truncation; the size marker's source.
    pub original_size: usize,
}

/// An MCP server's elicitation, forwarded for surfacing to the user —
/// in production via `session/request_permission`, origin-attributed to
/// the server and semantically distinct from tool permission prompts.
/// Elicitation responses never write the grant store.
#[derive(Debug, Clone, PartialEq)]
pub struct Elicitation {
    /// The originating server: the explicit origin attribution.
    pub server: String,
    pub message: String,
    /// The form schema, verbatim; `None` for URL elicitations.
    pub requested_schema: Option<serde_json::Value>,
    /// The URL for URL elicitations; `None` for form elicitations.
    pub url: Option<String>,
}

/// What the user (via the ACP elicitation surface) answered. An
/// acceptance carries the submitted form values when the client
/// collected them.
#[derive(Debug, Clone, PartialEq)]
pub enum ElicitationResponse {
    Accept {
        content: Option<serde_json::Value>,
    },
    Decline,
    Cancel,
}

/// Receives forwarded elicitations. The ACP layer implements this over
/// the client connection; permission prompts and elicitations are
/// distinct surfaces — a grant record is never written from here.
pub trait ElicitationSink: Send + Sync + 'static {
    fn elicit(
        &self,
        elicitation: Elicitation,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = ElicitationResponse> + Send + '_>,
    >;
}

/// A sink that starts fail-closed and can be pointed at the negotiated
/// connection's ACP bridge once `initialize` reveals the client's
/// elicitation capabilities.
#[derive(Default)]
pub struct DynamicElicitationSink {
    inner: std::sync::Mutex<Option<Arc<dyn ElicitationSink>>>,
}

impl DynamicElicitationSink {
    /// Routes subsequent elicitations to `sink`.
    pub fn set(&self, sink: Arc<dyn ElicitationSink>) {
        *self.inner.lock().expect("elicitation sink poisoned") = Some(sink);
    }
}

impl ElicitationSink for DynamicElicitationSink {
    fn elicit(
        &self,
        elicitation: Elicitation,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = ElicitationResponse> + Send + '_>,
    > {
        let inner = self
            .inner
            .lock()
            .expect("elicitation sink poisoned")
            .clone();
        Box::pin(async move {
            match inner {
                Some(sink) => sink.elicit(elicitation).await,
                None => ElicitationResponse::Decline,
            }
        })
    }
}

/// The fail-closed default: every elicitation is declined when no
/// forwarding surface is wired.
#[derive(Debug, Default, Clone, Copy)]
pub struct AutoDecline;

impl ElicitationSink for AutoDecline {
    fn elicit(
        &self,
        _elicitation: Elicitation,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = ElicitationResponse> + Send + '_>,
    > {
        Box::pin(async { ElicitationResponse::Decline })
    }
}

/// The pool's client handler: advertises elicitation support and
/// forwards every elicitation to the sink, attributed to its server.
#[derive(Clone)]
pub(crate) struct ForwardingHandler {
    sink: Arc<dyn ElicitationSink>,
    server: String,
}

impl ClientHandler for ForwardingHandler {
    fn get_info(&self) -> ClientConfig {
        let capabilities = ClientCapabilities::builder().enable_elicitation().build();
        ClientConfig::new(
            capabilities,
            Implementation::new("agentkit-tackle", env!("CARGO_PKG_VERSION")),
        )
    }

    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, rmcp::ErrorData> {
        let elicitation = match request {
            ElicitRequestParams::FormElicitationParams {
                message,
                requested_schema,
                ..
            } => Elicitation {
                server: self.server.clone(),
                message,
                requested_schema: serde_json::to_value(requested_schema).ok(),
                url: None,
            },
            ElicitRequestParams::UrlElicitationParams { message, url, .. } => Elicitation {
                server: self.server.clone(),
                message,
                requested_schema: None,
                url: Some(url),
            },
            // Non-exhaustive enum: unknown future elicitation modes
            // decline — fail-closed.
            _ => {
                return Ok(ElicitResult::new(ElicitationAction::Decline));
            }
        };
        let response = self.sink.elicit(elicitation).await;
        Ok(match response {
            ElicitationResponse::Accept { content } => match content {
                Some(content) => ElicitResult::new(ElicitationAction::Accept)
                    .with_content(content),
                None => ElicitResult::new(ElicitationAction::Accept),
            },
            ElicitationResponse::Decline => ElicitResult::new(ElicitationAction::Decline),
            ElicitationResponse::Cancel => ElicitResult::new(ElicitationAction::Cancel),
        })
    }
}
pub struct McpPool {
    connections: BTreeMap<String, Connection>,
    statuses: BTreeMap<String, ServerStatus>,
    pub enabled: BTreeMap<String, bool>,
    /// The registered servers' spawn configuration, by name. Held so
    /// re-enabling a server can reconnect it without the caller
    /// re-supplying the config.
    configs: BTreeMap<String, McpServerConfig>,
    /// Config-declared servers shadowed by a client-forwarded server of
    /// the same name (the client-declared server wins). Reported
    /// at the first turn.
    shadowed: BTreeSet<String>,
    sink: Arc<dyn ElicitationSink>,
    /// The helper that resolves `{cred:IDENTITY}` env markers at spawn.
    credential_helper: String,
}

struct Connection {
    peer: RunningService<RoleClient, ForwardingHandler>,
}

impl Default for McpPool {
    fn default() -> Self {
        Self::new()
    }
}

impl McpPool {
    /// An empty pool with the fail-closed auto-decline sink.
    pub fn new() -> Self {
        Self::with_sink(Arc::new(AutoDecline))
    }

    /// An empty pool forwarding elicitations to `sink`. The sink can be a
    /// dynamic handle the ACP layer points at the negotiated connection.
    pub fn with_sink(sink: Arc<dyn ElicitationSink>) -> Self {
        Self {
            connections: BTreeMap::new(),
            statuses: BTreeMap::new(),
            enabled: BTreeMap::new(),
            configs: BTreeMap::new(),
            shadowed: BTreeSet::new(),
            sink,
            credential_helper: crate::credentials::DEFAULT_CREDENTIAL_HELPER.to_owned(),
        }
    }

    /// Overrides the helper that resolves `{cred:IDENTITY}` env
    /// markers at spawn; wiring passes the configured value via
    /// [`crate::credentials::credential_helper_name`]. Defaults to
    /// [`crate::credentials::DEFAULT_CREDENTIAL_HELPER`].
    pub fn with_credential_helper(mut self, helper: impl Into<String>) -> Self {
        self.credential_helper = helper.into();
        self
    }

    /// Declares a configured server, enabled. Registering is what makes
    /// a server name addressable at all: toggling an unregistered name
    /// is an error rather than a silent no-op. Re-registering a name
    /// keeps its current enabled state, so a configuration reload does
    /// not silently re-enable a server the user turned off.
    pub fn register(&mut self, name: &str, config: McpServerConfig) {
        self.configs.insert(name.to_owned(), config);
        self.enabled.entry(name.to_owned()).or_insert(true);
    }

    /// The registered server names, in name order.
    pub fn server_names(&self) -> Vec<&str> {
        self.configs.keys().map(String::as_str).collect()
    }

    /// Moves another pool's connections and statuses into this one,
    /// leaving this pool's registered servers and enabled flags alone.
    /// Startup connects servers concurrently on throwaway single-server
    /// pools — `connect` needs unique access — and folds the results in
    /// here, so one dead server cannot hold up the rest.
    pub fn absorb(&mut self, other: Self) {
        self.connections.extend(other.connections);
        self.statuses.extend(other.statuses);
    }

    /// Statuses for the first-turn report: every declared server with its
    /// connected/failed state and enabled/disabled state.
    pub fn statuses(&self) -> BTreeMap<String, (ServerStatus, bool)> {
        let mut status_map = BTreeMap::new();
        for (name, status) in &self.statuses {
            status_map.insert(
                name.clone(),
                (status.clone(), *self.enabled.get(name).unwrap_or(&true)),
            );
        }
        // Registered servers that have no status yet: configured, never
        // connected (or re-enabled without a successful reconnect).
        for (name, &enabled) in &self.enabled {
            if !status_map.contains_key(name) {
                status_map.insert(
                    name.clone(),
                    (
                        ServerStatus::Failed {
                            reason: "Not connected".to_owned(),
                        },
                        enabled,
                    ),
                );
            }
        }
        status_map
    }

    /// Connects one configured server (stdio argv or HTTP URL), recording
    /// the outcome as status. Never panics; failures are statuses. A
    /// server that is explicitly disabled is not connected.
    pub async fn connect(&mut self, name: &str, config: &McpServerConfig) {
        if self.enabled.get(name) == Some(&false) {
            return;
        }

        let handler = ForwardingHandler {
            sink: Arc::clone(&self.sink),
            server: name.to_owned(),
        };
        let result: Result<RunningService<RoleClient, ForwardingHandler>, String> = match config {
            McpServerConfig::Stdio { command, args, env } => {
                self.connect_stdio(handler, name, command, args, env).await
            }
            McpServerConfig::Http { url, headers } => {
                self.connect_http(handler, url, headers).await
            }
        };
        match result {
            Ok(peer) => {
                self.connections
                    .insert(name.to_owned(), Connection { peer });
                self.statuses
                    .insert(name.to_owned(), ServerStatus::Connected);
            }
            Err(reason) => {
                tracing::warn!(
                    server = %name,
                    reason = %reason,
                    "MCP server failed to connect"
                );
                self.statuses
                    .insert(name.to_owned(), ServerStatus::Failed { reason });
            }
        }
    }

    /// Spawns a stdio MCP server and completes its initialize handshake.
    async fn connect_stdio(
        &self,
        handler: ForwardingHandler,
        name: &str,
        command: &str,
        args: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<RunningService<RoleClient, ForwardingHandler>, String> {
        let mut cmd = Command::new(command);
        configure_stdio_spawn(&mut cmd, command, args, env, &self.credential_helper)
            .map_err(|err| err.to_string())?;
        let (transport, stderr) = rmcp::transport::TokioChildProcess::builder(cmd)
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|err| err.to_string())?;
        if let Some(stderr) = stderr {
            let name = name.to_owned();
            tokio::spawn(async move {
                let mut cap = StderrRateCap::new(STDERR_RELAY_MAX_LINES, STDERR_RELAY_WINDOW);
                let mut out = tokio::io::stderr();
                let _ = relay_stderr(&name, stderr, &mut out, &mut cap).await;
            });
        }
        match connect_timeout(rmcp::service::serve_client(handler, transport)).await {
            Ok(initialised) => initialised.map_err(|err| err.to_string()),
            Err(reason) => Err(reason),
        }
    }

    /// Builds an HTTP MCP transport with the configured headers and
    /// completes its initialize handshake.
    async fn connect_http(
        &self,
        handler: ForwardingHandler,
        url: &str,
        headers: &BTreeMap<String, String>,
    ) -> Result<RunningService<RoleClient, ForwardingHandler>, String> {
        let custom_headers = headers
            .iter()
            .map(|(name, value)| {
                let name = http::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|err| format!("invalid header name `{name}`: {err}"))?;
                let value = http::HeaderValue::from_str(value)
                    .map_err(|err| format!("invalid value for header `{name}`: {err}"))?;
                Ok((name, value))
            })
            .collect::<Result<std::collections::HashMap<_, _>, String>>()?;
        let config = rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(url.to_owned())
            .custom_headers(custom_headers);
        let transport = rmcp::transport::StreamableHttpClientTransport::from_config(config);
        match connect_timeout(rmcp::service::serve_client(handler, transport)).await {
            Ok(initialised) => initialised.map_err(|err| err.to_string()),
            Err(reason) => Err(reason),
        }
    }

    /// Registers and connects servers forwarded by the ACP client on
    /// `session/new`, `session/load`, or `session/resume`. The
    /// client-declared server wins: a config-declared server of the same
    /// name is shadowed (and recorded for the first-turn report), its
    /// registration and connection replaced. A connection failure is a
    /// status, never an error, so a broken forwarded server cannot fail
    /// session setup.
    pub async fn add_forwarded(&mut self, servers: Vec<(String, McpServerConfig)>) {
        let throwaways = self.register_forwarded(servers);
        let connecting = throwaways
            .into_iter()
            .map(|(name, config, mut one)| async move {
                one.connect(&name, &config).await;
                one
            })
            .collect::<Vec<_>>();
        for connected in futures::future::join_all(connecting).await {
            self.absorb(connected);
        }
    }

    /// Registers forwarded servers and returns a throwaway pool per
    /// server, so the handshakes can run concurrently and, for the ACP
    /// path, outside this pool's lock.
    fn register_forwarded(
        &mut self,
        servers: Vec<(String, McpServerConfig)>,
    ) -> Vec<(String, McpServerConfig, Self)> {
        let mut throwaways = Vec::new();
        for (name, config) in servers {
            if self.configs.contains_key(&name) {
                self.shadowed.insert(name.clone());
            }
            self.register(&name, config.clone());
            // The client-declared server replaces any existing
            // registration, including a live config connection of the
            // same name.
            self.connections.remove(&name);
            throwaways.push((name, config, self.empty_sibling()));
        }
        throwaways
    }

    /// An empty pool sharing this pool's sink and credential helper.
    fn empty_sibling(&self) -> Self {
        Self {
            connections: BTreeMap::new(),
            statuses: BTreeMap::new(),
            enabled: BTreeMap::new(),
            configs: BTreeMap::new(),
            shadowed: BTreeSet::new(),
            sink: Arc::clone(&self.sink),
            credential_helper: self.credential_helper.clone(),
        }
    }

    /// Config-declared servers shadowed by a client-forwarded server of
    /// the same name, for the first-turn report.
    pub fn shadowed_servers(&self) -> Vec<String> {
        self.shadowed.iter().cloned().collect()
    }

    /// Whether a server is connected (its tools are callable).
    pub fn is_connected(&self, name: &str) -> bool {
        self.connections.contains_key(name)
    }

    /// Whether a registered server is enabled. Unregistered names are
    /// not addressable, so they read as disabled.
    pub fn is_enabled(&self, name: &str) -> bool {
        *self.enabled.get(name).unwrap_or(&false)
    }

    /// Enables a server and attempts to reconnect it, restoring the
    /// connection state a disable tore down. A connection failure is a
    /// status, not an error: the server stays enabled and
    /// [`Self::statuses`] carries the reason.
    pub async fn enable_server(&mut self, name: &str) -> Result<(), McpPoolError> {
        self.require_registered(name)?;
        self.enabled.insert(name.to_owned(), true);
        if let Some(config) = self.configs.get(name).cloned() {
            self.connect(name, &config).await;
        }
        Ok(())
    }

    /// Disables a server: no further connections, and any live
    /// connection is closed. Disabling an already-disabled server is a
    /// no-op.
    pub fn disable_server(&mut self, name: &str) -> Result<(), McpPoolError> {
        self.require_registered(name)?;
        self.enabled.insert(name.to_owned(), false);
        self.connections.remove(name);
        self.statuses.remove(name);
        Ok(())
    }

    /// Toggles a server, returning the new enabled state. Enabling
    /// reconnects, per [`Self::enable_server`].
    pub async fn toggle_server(&mut self, name: &str) -> Result<bool, McpPoolError> {
        if self.is_enabled(name) {
            self.disable_server(name)?;
            Ok(false)
        } else {
            self.enable_server(name).await?;
            Ok(true)
        }
    }

    fn require_registered(&self, name: &str) -> Result<(), McpPoolError> {
        if self.configs.contains_key(name) {
            Ok(())
        } else {
            Err(McpPoolError::UnknownServer(name.to_owned()))
        }
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
            _ => {
                return Err(crate::mcp::McpPoolError::Call(
                    "tool arguments must be a JSON object".to_owned(),
                ));
            }
        };
        let mut params = CallToolRequestParams::new(tool.to_owned());
        params.arguments = arguments;
        let result = connection
            .peer
            .call_tool(params)
            .await
            .map_err(|err| crate::mcp::McpPoolError::Call(err.to_string()))?;

        let content_json =
            serde_json::to_string(&result.content).unwrap_or_else(|_| "[]".to_owned());
        let original_size = content_json.len();
        let mut content_json = content_json;
        let mut truncated = false;
        if content_json.len() > TOOL_RESULT_LIMIT {
            // Truncate at a character boundary: multi-byte content
            // must not panic the harness.
            let mut end = TOOL_RESULT_LIMIT;
            while !content_json.is_char_boundary(end) {
                end -= 1;
            }
            content_json.truncate(end);
            truncated = true;
        }
        Ok(ToolResult {
            content_json,
            is_error: result.is_error.unwrap_or(false),
            truncated,
            original_size,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum McpPoolError {
    #[error("MCP server `{0}` is not connected")]
    ServerNotConnected(String),
    #[error("no MCP server named `{0}` is configured")]
    UnknownServer(String),
    #[error("tool call failed: {0}")]
    Call(String),
}

/// Connects forwarded servers concurrently on throwaway pools and folds
/// the results in, without holding `pool`'s lock during the handshakes.
/// A slow or dead forwarded server therefore cannot serialise every other
/// session's tool listing behind it.
pub async fn connect_forwarded(
    pool: Arc<tokio::sync::Mutex<McpPool>>,
    servers: Vec<(String, McpServerConfig)>,
) {
    let throwaways = {
        let mut pool = pool.lock().await;
        pool.register_forwarded(servers)
    };
    if throwaways.is_empty() {
        return;
    }
    let connected = futures::future::join_all(
        throwaways
            .into_iter()
            .map(|(name, config, mut one)| async move {
                one.connect(&name, &config).await;
                one
            }),
    )
    .await;
    let mut pool = pool.lock().await;
    for one in connected {
        pool.absorb(one);
    }
}
