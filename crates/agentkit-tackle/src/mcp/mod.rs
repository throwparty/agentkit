//! The MCP client pool: connections to configured and client-provided
//! servers, tools namespaced `mcp.<server>.<tool>`, per-server status.
//!
//! Connection is asynchronous with a per-server timeout, so session
//! creation never blocks; the status of every server (connected/failed)
//! is reported at the first turn. Nothing about which servers exist is
//! hardcoded.
//!
//! Spawn hygiene: stdio servers start with explicit argv (no shell
//! interpretation) and only the named environment entries — tackle's
//! own environment is never forwarded. A whole env value of the form
//! `{cred:IDENTITY}` resolves through the credential helper at spawn
//! (never at config load), so secrets never enter `Config`. Child
//! stderr is relayed to tackle's stderr line-prefixed with the server
//! name under a rate cap; the child's stdout is the MCP protocol
//! channel and is never touched by the relay.

use crate::config::McpServerConfig;
use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientConfig, ElicitRequestParams, ElicitResult,
    ElicitationAction, Implementation,
};
use rmcp::service::{RequestContext, RoleClient, RunningService};
use rmcp::ClientHandler;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// Tool results are truncated at this size; full content is what the
/// server sent (the truncation is a context/cost guard).
pub const TOOL_RESULT_LIMIT: usize = 16 * 1024;

/// Stderr relay rate cap: at most this many lines per relay window.
pub const STDERR_RELAY_MAX_LINES: usize = 10;

/// The relay window the stderr rate cap applies to.
pub const STDERR_RELAY_WINDOW: Duration = Duration::from_secs(1);

/// Connection timeout per server: session creation must never block.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

async fn connect_timeout<T>(future: impl std::future::Future<Output = T>) -> Result<T, String> {
    tokio::time::timeout(CONNECT_TIMEOUT, future)
        .await
        .map_err(|_| "connection exceeded the 10s per-server timeout".to_owned())
}

/// The spawn-hygiene operations, abstracted so construction is
/// unit-testable against a recording fake.
pub trait ConfigureCommand {
    fn set_program(&mut self, program: &str);
    fn set_args(&mut self, args: &[String]);
    fn clear_env(&mut self);
    fn set_env(&mut self, key: &str, value: &str);
}

impl ConfigureCommand for Command {
    /// Only `Command::new` can set the program, so this replaces the
    /// command; `configure_stdio_spawn` calls it before anything else.
    fn set_program(&mut self, program: &str) {
        *self = Command::new(program);
    }

    fn set_args(&mut self, args: &[String]) {
        self.args(args);
    }

    fn clear_env(&mut self) {
        self.env_clear();
    }

    fn set_env(&mut self, key: &str, value: &str) {
        self.env(key, value);
    }
}

/// An env value failed to resolve for a stdio spawn.
#[derive(Debug, thiserror::Error)]
pub enum EnvValueError {
    /// A value containing the marker was not the marker alone.
    #[error("env `{key}`: a `{{cred:...}}` marker must be the entire value")]
    MarkerNotEntire {
        key: String,
    },
    /// The marker carried no identity.
    #[error("env `{key}`: the `{{cred:...}}` marker has an empty identity")]
    EmptyIdentity {
        key: String,
    },
    /// The credential helper failed to produce the secret.
    #[error("env `{key}`: {source}")]
    Credential {
        key: String,
        #[source]
        source: crate::agent::provider::CredentialError,
    },
}

/// The `{cred:IDENTITY}` marker prefix: a whole env value of this form
/// resolves through the credential helper at spawn.
const CRED_MARKER_PREFIX: &str = "{cred:";

/// Resolves one env value for a spawn: a value that is the entire
/// `{cred:IDENTITY}` marker resolves through `credential_helper`
/// (component [`crate::agent::provider::CREDENTIAL_COMPONENT`]); any
/// value without the marker passes through verbatim. Resolution runs
/// at spawn, never at config load, so secrets never enter `Config`.
fn resolve_env_value(
    credential_helper: &str,
    key: &str,
    value: &str,
) -> Result<String, EnvValueError> {
    if !value.contains(CRED_MARKER_PREFIX) {
        return Ok(value.to_owned());
    }
    let Some(rest) = value.strip_prefix(CRED_MARKER_PREFIX) else {
        return Err(EnvValueError::MarkerNotEntire {
            key: key.to_owned(),
        });
    };
    let Some(identity) = rest.strip_suffix('}') else {
        return Err(EnvValueError::MarkerNotEntire {
            key: key.to_owned(),
        });
    };
    if identity.is_empty() {
        return Err(EnvValueError::EmptyIdentity {
            key: key.to_owned(),
        });
    }
    crate::agent::provider::resolve_credential_value(credential_helper, identity).map_err(
        |source| EnvValueError::Credential {
            key: key.to_owned(),
            source,
        },
    )
}

/// Applies spawn hygiene to a stdio server spawn: explicit argv (no
/// shell interpretation) and only the named environment entries —
/// no blanket forwarding of tackle's own environment. Env values
/// resolve first: a failed `{cred:IDENTITY}` resolution returns
/// before the command is mutated at all.
pub fn configure_stdio_spawn(
    cmd: &mut impl ConfigureCommand,
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    credential_helper: &str,
) -> Result<(), EnvValueError> {
    let resolved = env
        .iter()
        .map(|(key, value)| {
            resolve_env_value(credential_helper, key, value).map(|value| (key, value))
        })
        .collect::<Result<Vec<_>, _>>()?;
    cmd.set_program(command);
    cmd.set_args(args);
    cmd.clear_env();
    for (key, value) in resolved {
        cmd.set_env(key, &value);
    }
    Ok(())
}

/// A rate-cap admission decision for one stderr line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Relay,
    Drop,
    /// The relay window rolled over after drops; report the count once.
    WindowReset {
        dropped: usize,
    },
}

/// The stderr relay rate cap: at most `max_per_window` lines per
/// `window`, dropping the rest. Time is injected so tests are
/// deterministic.
pub struct StderrRateCap {
    max_per_window: usize,
    window: Duration,
    window_start: std::time::Instant,
    relayed: usize,
    dropped: usize,
}

impl StderrRateCap {
    pub fn new(max_per_window: usize, window: Duration) -> Self {
        Self {
            max_per_window,
            window,
            window_start: std::time::Instant::now(),
            relayed: 0,
            dropped: 0,
        }
    }

    pub fn admit(&mut self, now: std::time::Instant) -> Admission {
        if now.duration_since(self.window_start) >= self.window {
            let dropped = std::mem::take(&mut self.dropped);
            self.window_start = now;
            self.relayed = 0;
            if dropped > 0 {
                return Admission::WindowReset { dropped };
            }
        }
        if self.relayed < self.max_per_window {
            self.relayed += 1;
            Admission::Relay
        } else {
            self.dropped += 1;
            Admission::Drop
        }
    }
}

/// Relays child stderr to `out` (tackle's stderr in production) with
/// each line prefixed by the server name under the rate cap. The
/// child's stdout is never seen here: it is the MCP protocol channel.
pub async fn relay_stderr<S, W>(
    server: &str,
    stderr: S,
    out: &mut W,
    cap: &mut StderrRateCap,
) -> std::io::Result<()>
where
    S: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = BufReader::new(stderr).lines();
    while let Some(line) = lines.next_line().await? {
        match cap.admit(std::time::Instant::now()) {
            Admission::Relay => {
                out.write_all(format!("mcp[{server}] {line}\n").as_bytes())
                    .await?;
            }
            Admission::WindowReset { dropped } => {
                out.write_all(
                    format!("mcp[{server}] [rate cap: dropped {dropped} stderr lines]\n")
                        .as_bytes(),
                )
                .await?;
                out.write_all(format!("mcp[{server}] {line}\n").as_bytes())
                    .await?;
            }
            Admission::Drop => {}
        }
    }
    Ok(())
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

/// What the user (via the ACP permission surface) answered. A
/// permission prompt cannot collect form data, so an acceptance
/// carries no content — servers design around that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElicitationResponse {
    Accept,
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
    ) -> impl std::future::Future<Output = ElicitationResponse> + Send;
}

/// The fail-closed default: every elicitation is declined when no
/// forwarding surface is wired.
#[derive(Debug, Default, Clone, Copy)]
pub struct AutoDecline;

impl ElicitationSink for AutoDecline {
    async fn elicit(&self, _elicitation: Elicitation) -> ElicitationResponse {
        ElicitationResponse::Decline
    }
}

/// The pool's client handler: advertises elicitation support and
/// forwards every elicitation to the sink, attributed to its server.
#[derive(Clone)]
pub(crate) struct ForwardingHandler<S: ElicitationSink> {
    sink: Arc<S>,
    server: String,
}

impl<S: ElicitationSink> ClientHandler for ForwardingHandler<S> {
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
            ElicitationResponse::Accept => ElicitResult::new(ElicitationAction::Accept),
            ElicitationResponse::Decline => ElicitResult::new(ElicitationAction::Decline),
            ElicitationResponse::Cancel => ElicitResult::new(ElicitationAction::Cancel),
        })
    }
}
pub struct McpPool<S: ElicitationSink = AutoDecline> {
    connections: BTreeMap<String, Connection<S>>,
    statuses: BTreeMap<String, ServerStatus>,
    enabled: BTreeMap<String, bool>,
    sink: Arc<S>,
    /// The helper that resolves `{cred:IDENTITY}` env markers at spawn.
    credential_helper: String,
}

struct Connection<S: ElicitationSink> {
    peer: RunningService<RoleClient, ForwardingHandler<S>>,
}

impl Default for McpPool<AutoDecline> {
    fn default() -> Self {
        Self::new()
    }
}

impl McpPool<AutoDecline> {
    /// An empty pool with the fail-closed auto-decline sink.
    pub fn new() -> Self {
        Self::with_sink(AutoDecline)
    }
}

impl<S: ElicitationSink> McpPool<S> {
    /// An empty pool forwarding elicitations to `sink`.
    pub fn with_sink(sink: S) -> Self {
        Self {
            connections: BTreeMap::new(),
            statuses: BTreeMap::new(),
            enabled: BTreeMap::new(),
            sink: Arc::new(sink),
            credential_helper: crate::agent::provider::DEFAULT_CREDENTIAL_HELPER.to_owned(),
        }
    }

    /// Overrides the helper that resolves `{cred:IDENTITY}` env
    /// markers at spawn; wiring passes the configured value via
    /// [`crate::agent::provider::credential_helper_name`]. Defaults to
    /// [`crate::agent::provider::DEFAULT_CREDENTIAL_HELPER`].
    pub fn with_credential_helper(mut self, helper: impl Into<String>) -> Self {
        self.credential_helper = helper.into();
        self
    }

    /// Statuses for the first-turn report: every declared server with its
    /// connected/failed state.
    pub fn statuses(&self) -> &BTreeMap<String, ServerStatus> {
        &self.statuses
    }

    /// Connects one configured server (stdio argv or HTTP URL), recording
    /// the outcome as status. Never panics; failures are statuses.
    pub async fn connect(&mut self, name: &str, config: &McpServerConfig) {
        // Skip connection if server is disabled
        if !self.enabled.get(name).unwrap_or(&true) {
            return;
        }
        
        let handler = ForwardingHandler {
            sink: Arc::clone(&self.sink),
            server: name.to_owned(),
        };
        let result: Result<RunningService<RoleClient, ForwardingHandler<S>>, String> = async {
            match config {
                McpServerConfig::Stdio { command, args, env } => {
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
                            let mut cap =
                                StderrRateCap::new(STDERR_RELAY_MAX_LINES, STDERR_RELAY_WINDOW);
                            let mut out = tokio::io::stderr();
                            let _ = relay_stderr(&name, stderr, &mut out, &mut cap).await;
                        });
                    }
                    match connect_timeout(rmcp::service::serve_client(handler, transport)).await {
                        Ok(initialised) => initialised.map_err(|err| err.to_string()),
                        Err(reason) => Err(reason),
                    }
                }
                McpServerConfig::Http { url } => {
                    let transport =
                        rmcp::transport::StreamableHttpClientTransport::from_uri(url.clone());
                    match connect_timeout(rmcp::service::serve_client(handler, transport)).await {
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

    /// Whether a server is enabled for connection.
    pub fn is_enabled(&self, name: &str) -> bool {
        *self.enabled.get(name).unwrap_or(&true)
    }

    /// Enable a server for connection.
    pub fn enable_server(&mut self, name: &str) -> Result<(), McpPoolError> {
        self.enabled.insert(name.to_owned(), true);
        Ok(())
    }

    /// Disable a server, preventing new connections and closing existing ones.
    pub fn disable_server(&mut self, name: &str) -> Result<(), McpPoolError> {
        self.enabled.insert(name.to_owned(), false);
        // Close existing connection if any
        self.connections.remove(name);
        self.statuses.remove(name);
        Ok(())
    }

    /// Toggle a server's enabled state, returning the new state.
    pub fn toggle_server(&mut self, name: &str) -> Result<bool, McpPoolError> {
        let current_state = *self.enabled.get(name).unwrap_or(&true);
        let new_state = !current_state;
        self.enabled.insert(name.to_owned(), new_state);
        
        // If disabling and there's an active connection, close it
        if !new_state {
            self.connections.remove(name);
            self.statuses.remove(name);
        }
        
        Ok(new_state)
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
                ))
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
    #[error("tool call failed: {0}")]
    Call(String),
}

#[cfg(test)]
mod spawn_hygiene_tests {
    use super::*;

    /// Records the spawn-hygiene calls in order, including the env
    /// clear, so argv construction and env filtering are assertable.
    #[derive(Default)]
    struct RecordingCommand {
        program: Option<String>,
        args: Vec<String>,
        cleared: bool,
        env: BTreeMap<String, String>,
        env_set_after_clear: Vec<(String, String)>,
    }

    impl ConfigureCommand for RecordingCommand {
        fn set_program(&mut self, program: &str) {
            self.program = Some(program.to_owned());
        }

        fn set_args(&mut self, args: &[String]) {
            self.args = args.to_vec();
        }

        fn clear_env(&mut self) {
            self.cleared = true;
        }

        fn set_env(&mut self, key: &str, value: &str) {
            self.env.insert(key.to_owned(), value.to_owned());
            self.env_set_after_clear
                .push((key.to_owned(), value.to_owned()));
        }
    }

    #[test]
    fn argv_construction_is_explicit_without_a_shell() {
        let mut cmd = RecordingCommand::default();
        configure_stdio_spawn(
            &mut cmd,
            "mcp-server-everything",
            &["--verbose".into()],
            &BTreeMap::new(),
            "keychain",
        )
        .unwrap();
        assert_eq!(cmd.program.as_deref(), Some("mcp-server-everything"));
        assert_eq!(cmd.args, vec!["--verbose".to_owned()]);
        // No shell: the program name is the argv[0] verbatim, never a
        // shell command line.
        assert!(!cmd.program.as_deref().unwrap_or_default().contains(' '));
    }

    #[test]
    fn env_is_cleared_then_only_named_entries_set() {
        let mut env = BTreeMap::new();
        env.insert("MCP_TOKEN".to_owned(), "secret".to_owned());
        env.insert("PATH".to_owned(), "/custom/bin".to_owned());

        let mut cmd = RecordingCommand::default();
        configure_stdio_spawn(&mut cmd, "server", &[], &env, "keychain").unwrap();

        assert!(cmd.cleared, "no blanket environment forwarding");
        assert_eq!(cmd.env, env);
        // Entries are set only after the clear, so the parent's env is
        // never forwarded.
        assert_eq!(cmd.env_set_after_clear.len(), 2);
    }

    /// Writes an `agentkit-credential-test` helper that rejects any
    /// invocation missing the `get tackle <identity>` shape, like the
    /// shipped helpers do.
    fn write_fake_helper(dir: &std::path::Path, body: &str) {
        let path = dir.join("agentkit-credential-test");
        let mut file = std::fs::File::create(&path).unwrap();
        use std::io::Write as _;
        writeln!(
            file,
            "#!/bin/sh\n\
             [ \"$#\" -eq 3 ] && [ \"$1\" = get ] && [ \"$2\" = tackle ] || exit 3\n\
             echo '{body}'"
        )
        .unwrap();
        file.flush().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn cred_markers_resolve_through_the_credential_helper() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_helper(dir.path(), "{\"access_token\": \"tok\"}");

        let mut env = BTreeMap::new();
        env.insert("GITHUB_TOKEN".to_owned(), "{cred:github}".to_owned());
        env.insert("LITERAL".to_owned(), "plain-value".to_owned());

        let mut cmd = RecordingCommand::default();
        let _guard = crate::agent::provider::PATH_LOCK.lock().unwrap();
        let path_var = std::env::var("PATH").unwrap();
        std::env::set_var("PATH", format!("{}:{}", dir.path().display(), path_var));
        let resolved = configure_stdio_spawn(&mut cmd, "server", &[], &env, "test");
        std::env::set_var("PATH", path_var);

        resolved.unwrap();
        assert_eq!(
            cmd.env.get("GITHUB_TOKEN").map(String::as_str),
            Some("tok"),
        );
        assert_eq!(
            cmd.env.get("LITERAL").map(String::as_str),
            Some("plain-value"),
        );
    }

    #[test]
    fn a_failed_cred_resolution_leaves_the_command_untouched() {
        let mut env = BTreeMap::new();
        env.insert("GITHUB_TOKEN".to_owned(), "{cred:github}".to_owned());

        let mut cmd = RecordingCommand::default();
        let err = configure_stdio_spawn(
            &mut cmd,
            "server",
            &[],
            &env,
            "definitely-missing-helper",
        )
        .unwrap_err();

        assert!(err.to_string().contains("GITHUB_TOKEN"), "{err}");
        // Resolution happens before any mutation: a failure never
        // half-spawns with an unresolved secret.
        assert_eq!(cmd.program, None, "the command was not configured");
        assert!(cmd.env.is_empty());
    }

    #[test]
    fn an_empty_cred_identity_is_rejected() {
        let mut env = BTreeMap::new();
        env.insert("GITHUB_TOKEN".to_owned(), "{cred:}".to_owned());

        let mut cmd = RecordingCommand::default();
        let err = configure_stdio_spawn(&mut cmd, "server", &[], &env, "test").unwrap_err();
        assert!(err.to_string().contains("empty identity"), "{err}");
    }

    #[test]
    fn a_cred_marker_must_be_the_entire_value() {
        let mut env = BTreeMap::new();
        env.insert("AUTH".to_owned(), "Bearer {cred:github}".to_owned());

        let mut cmd = RecordingCommand::default();
        let err = configure_stdio_spawn(&mut cmd, "server", &[], &env, "test").unwrap_err();
        assert!(err.to_string().contains("entire value"), "{err}");
    }

    #[tokio::test]
    async fn stderr_relay_prefixes_lines_with_the_server_name() {
        let (mut client, server) = tokio::io::duplex(64);
        client.write_all(b"hello\nworld\n").await.unwrap();
        drop(client);

        let mut out = Vec::new();
        let mut cap = StderrRateCap::new(STDERR_RELAY_MAX_LINES, STDERR_RELAY_WINDOW);
        relay_stderr("echo", server, &mut out, &mut cap)
            .await
            .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert_eq!(text, "mcp[echo] hello\nmcp[echo] world\n");
    }

    #[tokio::test]
    async fn stderr_relay_is_rate_capped_within_a_window() {
        let (mut client, server) = tokio::io::duplex(512);
        for i in 0..20 {
            client
                .write_all(format!("line {i}\n").as_bytes())
                .await
                .unwrap();
        }
        drop(client);

        let mut out = Vec::new();
        // A fresh cap in a young window: at most 5 lines survive.
        let mut cap = StderrRateCap::new(5, STDERR_RELAY_WINDOW);
        relay_stderr("echo", server, &mut out, &mut cap)
            .await
            .unwrap();

        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5, "only the capped number of lines relayed");
        assert!(lines.iter().all(|line| line.starts_with("mcp[echo] ")));
    }

    #[test]
    fn rate_cap_reports_dropped_lines_on_window_reset() {
        let start = std::time::Instant::now();
        let mut cap = StderrRateCap::new(2, Duration::from_secs(10));

        assert_eq!(cap.admit(start), Admission::Relay);
        assert_eq!(cap.admit(start), Admission::Relay);
        assert_eq!(cap.admit(start), Admission::Drop);
        assert_eq!(cap.admit(start), Admission::Drop);

        // After the window rolls over the cap lifts and the dropped
        // count is reported once.
        let later = start + Duration::from_secs(11);
        assert_eq!(cap.admit(later), Admission::WindowReset { dropped: 2 });
        assert_eq!(cap.admit(later), Admission::Relay);
    }
}
