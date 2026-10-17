//! ACP server surface: protocol handling via the SDK's callback builders.
//!
//! Handlers are registered per request type and tried in order; shared
//! state rides in `Arc<TackleState>` captured by each callback, and
//! per-connection negotiation state in `Arc<ConnectionNegotiation>`. The
//! stdio transport is the normal launch mode (one process per client
//! connection); HTTP arrives with T-036.

use crate::config::Loaded;
use crate::loader::Definitions;
use crate::mcp::{McpPool, ServerStatus};
use crate::store::SessionStore;

pub mod client_tools;
pub mod compaction;
pub mod config_options;
pub mod first_run;
pub mod fork;
pub mod http;
pub mod titling;
use agent_client_protocol::schema::v1::{
    AgentCapabilities, AvailableCommandsUpdate, CloseSessionResponse, DeleteSessionResponse,
    InitializeRequest, InitializeResponse, ListSessionsResponse, LoadSessionRequest,
    LoadSessionResponse, McpCapabilities, NewSessionRequest, NewSessionResponse,
    Notice, NoticeSeverity, PromptCapabilities, ResumeSessionRequest, ResumeSessionResponse,
    SessionCapabilities,
    SessionCloseCapabilities, SessionDeleteCapabilities, SessionForkCapabilities, SessionId,
    SessionInfo, SessionListCapabilities, SessionNotification, SessionResumeCapabilities,
    SessionUpdate,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{Agent, Error, Stdio};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

/// RFD-shaped features the client advertised support for. Unstable
/// updates are sent only for features captured here; stable-variant
/// fallbacks cover everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UnstableFeature {
    Fork,
    SessionCompaction,
    SessionNotices,
}

impl UnstableFeature {
    /// The `_meta` key advertising this feature, per the extensibility
    /// conventions (feature names double as capability keys).
    pub fn meta_key(&self) -> &'static str {
        match self {
            UnstableFeature::Fork => "unstable_session_fork",
            UnstableFeature::SessionCompaction => "unstable_session_compaction",
            UnstableFeature::SessionNotices => "unstable_session_notices",
        }
    }

    /// The client-advertised subset of unstable features, read from
    /// `clientCapabilities._meta`.
    pub fn advertised_in(meta: &Option<agent_client_protocol::schema::v1::Meta>) -> BTreeSet<Self> {
        let mut supported = BTreeSet::new();
        for feature in [Self::Fork, Self::SessionCompaction, Self::SessionNotices] {
            if meta
                .as_ref()
                .is_some_and(|meta| meta.contains_key(feature.meta_key()))
            {
                supported.insert(feature);
            }
        }
        supported
    }
}

/// Per-connection negotiation and client-surface state, captured or
/// seeded when the connection opens and consulted by every update sender.
pub struct ConnectionNegotiation {
    supported: Mutex<BTreeSet<UnstableFeature>>,
    /// The client's advertised ACP capabilities (fs/terminal surface),
    /// the source of truth for which `client.*` tools exist.
    caps: Mutex<client_tools::ClientCaps>,
    /// The `/editor` toggles for the client-capability categories,
    /// seeded from `[client_tools]` and overridable at runtime.
    editor: Mutex<client_tools::EditorToggles>,
    /// Terminal ids the harness created for this connection, by session,
    /// for release at session close. Connection-scoped and ephemeral.
    terminals: Mutex<BTreeMap<String, BTreeSet<String>>>,
    /// The connection's MCP pool: the configured servers plus any the
    /// client forwarded, mutable via `/mcp`. Per-connection, so an
    /// availability toggle or a forwarded server never bleeds into
    /// another connection in the same process.
    mcp_pool: Arc<tokio::sync::Mutex<McpPool>>,
}

impl ConnectionNegotiation {
    /// A fresh connection's state, with the editor toggles seeded from
    /// configuration and its own MCP pool.
    pub fn new(editor: client_tools::EditorToggles, mcp_pool: McpPool) -> Self {
        Self {
            supported: Mutex::new(BTreeSet::new()),
            caps: Mutex::new(client_tools::ClientCaps::default()),
            editor: Mutex::new(editor),
            terminals: Mutex::new(BTreeMap::new()),
            mcp_pool: Arc::new(tokio::sync::Mutex::new(mcp_pool)),
        }
    }

    /// The connection's MCP pool handle.
    pub fn mcp_pool(&self) -> Arc<tokio::sync::Mutex<McpPool>> {
        Arc::clone(&self.mcp_pool)
    }

    fn capture(&self, capabilities: &agent_client_protocol::schema::v1::ClientCapabilities) {
        *self.supported.lock().expect("negotiation poisoned") =
            UnstableFeature::advertised_in(&capabilities.meta);
        *self.caps.lock().expect("negotiation poisoned") =
            client_tools::ClientCaps::from_capabilities(capabilities);
    }

    /// Whether the client advertised support for `feature`.
    pub fn supports(&self, feature: UnstableFeature) -> bool {
        self.supported
            .lock()
            .expect("negotiation poisoned")
            .contains(&feature)
    }

    /// The client's advertised capability surface.
    pub fn client_caps(&self) -> client_tools::ClientCaps {
        *self.caps.lock().expect("negotiation poisoned")
    }

    /// The editor toggles currently in effect for this connection.
    fn editor_toggles(&self) -> client_tools::EditorToggles {
        *self.editor.lock().expect("negotiation poisoned")
    }

    /// Records a terminal created for `session`, or drops one that has
    /// finished, so session close can release whatever remains.
    fn track_terminals(&self, session_id: &str, created: Option<String>, finished: Option<String>) {
        if created.is_none() && finished.is_none() {
            return;
        }
        let mut terminals = self.terminals.lock().expect("negotiation poisoned");
        if let Some(id) = created {
            terminals.entry(session_id.to_owned()).or_default().insert(id);
        }
        if let Some(id) = finished {
            if let Some(set) = terminals.get_mut(session_id) {
                set.remove(&id);
            }
        }
    }
}

/// State shared across the process's handlers: the session store, loaded
/// configuration, and definitions. Per-connection state (negotiation, the
/// client surface, its terminals, and its MCP pool) lives in
/// `ConnectionNegotiation` so HTTP connections never bleed into one
/// another.
pub struct TackleState {
    pub db: std::sync::Arc<SessionStore>,
    pub config: Loaded,
    pub definitions: Definitions,
    /// The in-memory permission grant store (FR-014): keyed by session
    /// and invokable, gone on close, process exit, and resume after
    /// restart.
    pub grants: tokio::sync::Mutex<crate::permissions::GrantStore>,
}

impl TackleState {
    /// The capability map: the stable v1 surface plus the unstable
    /// features tackle implements (advertised unconditionally — the gate
    /// applies to *sending* RFD-shaped updates, per FR-002).
    fn capabilities(&self) -> AgentCapabilities {
        let mut unstable: agent_client_protocol::schema::v1::Meta =
            agent_client_protocol::schema::v1::Meta::new();
        for feature in [
            UnstableFeature::Fork,
            UnstableFeature::SessionCompaction,
            UnstableFeature::SessionNotices,
        ] {
            unstable.insert(feature.meta_key().to_owned(), serde_json::json!({}));
        }
        AgentCapabilities::new()
            .load_session(true)
            .prompt_capabilities(
                PromptCapabilities::new()
                    .image(true)
                    .audio(false)
                    .embedded_context(true),
            )
            .mcp_capabilities(McpCapabilities::new().http(true).sse(false))
            .session_capabilities(
                SessionCapabilities::new()
                    .list(Some(SessionListCapabilities::default()))
                    .close(Some(SessionCloseCapabilities::default()))
                    .resume(Some(SessionResumeCapabilities::default()))
                    .delete(Some(SessionDeleteCapabilities::default()))
                    .fork(Some(SessionForkCapabilities::default())),
            )
            .meta(unstable)
    }
}

/// Prefixes a session id for the wire (the storage ADR's serialization
/// boundary).
fn wire_session_id(id: &crate::store::SessionId) -> SessionId {
    SessionId::new(format!("sess_{id}"))
}

/// The `available_commands_update` payload for a session: the registry
/// view over the loaded definitions — the `!` prefix command plus every
/// user-invokable prompt, with parameter hints from frontmatter. The
/// registry carries the pool's MCP tools here; individual tools are
/// never advertised as commands (only reached through `/!`).
///
/// The tool listing is per-connection state, not configuration, so it is
/// read live: a server disabled by `/mcp disable` drops out of the
/// listing for the rest of the connection.
async fn available_commands_update(
    state: &TackleState,
    negotiation: &ConnectionNegotiation,
) -> SessionUpdate {
    let pool = negotiation.mcp_pool();
    let pool = pool.lock().await;
    let mcp_tools = pool.list_tools().await;
    drop(pool);

    let client_tools = client_tools::catalog(
        negotiation.client_caps(),
        negotiation.editor_toggles(),
    );
    let registry = match crate::invokables::Registry::build_with_client_tools(
        &state.definitions,
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
fn forwarded_mcp_server(
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
async fn connect_forwarded_servers(
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
    let helper = crate::agent::provider::credential_helper_name(&state.config.config);
    let new_pool = || McpPool::new().with_credential_helper(helper.clone());
    let mut pool = new_pool();
    for (server_name, server_config) in &state.config.config.mcp_servers {
        pool.register(server_name, server_config.clone());
    }
    let connecting = state
        .config
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

/// The first-turn MCP report (FR-013): a warning notice naming enabled
/// servers that failed to connect and config servers shadowed by a
/// client-declared server of the same name. `None` when nothing is
/// wrong, so a clean session stays silent.
async fn mcp_status_notice(negotiation: &ConnectionNegotiation) -> Option<Notice> {
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

/// Strips the wire prefix back to the bare store id.
fn store_session_id(id: &SessionId) -> String {
    id.to_string()
        .strip_prefix("sess_")
        .unwrap_or(id.to_string().as_str())
        .to_owned()
}

/// One server's line in the `/mcp status` report. A disabled server
/// reads as disabled and carries no connection state: disabling closes
/// the connection, so a status alongside it would be noise.
fn mcp_status_line(name: &str, status: &ServerStatus, enabled: bool) -> String {
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
async fn mcp_command(negotiation: &ConnectionNegotiation, first_text: &str) -> Option<String> {
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
fn editor_command(negotiation: &ConnectionNegotiation, first_text: &str) -> Option<String> {
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

/// Runs the ACP agent over stdio until the transport closes.
pub async fn run_stdio(state: Arc<TackleState>) -> agent_client_protocol::Result<()> {
    let editor = client_tools::EditorToggles::from_config(&state.config.config);
    let pool = connect_config_servers(&state).await;
    let negotiation = Arc::new(ConnectionNegotiation::new(editor, pool));
    // Ctrl-C/SIGTERM ends the transport cleanly: the connection is
    // dropped and the caller reports a normal shutdown.
    tokio::select! {
        result = run_agent_over(state, negotiation, Stdio::new()) => result,
        _ = crate::cli::wait_for_shutdown() => Ok(()),
    }
}

/// Runs one agent connection over `transport`: the handler chain is
/// built per connection (shared state rides `Arc<TackleState>`), so
/// HTTP serves multiple connections in one process (T-036).
pub(crate) async fn run_agent_over<T>(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    transport: T,
) -> agent_client_protocol::Result<()>
where
    T: agent_client_protocol::ConnectTo<agent_client_protocol::role::acp::Agent>,
{
    let negotiation_for_init = negotiation.clone();
    let state_for_init = state.clone();

    let state_for_new = state.clone();
    let state_for_auth = state.clone();
    let state_for_list = state.clone();
    let negotiation_for_new = negotiation.clone();
    let negotiation_for_config = negotiation.clone();
    let state_for_config = state.clone();
    let state_for_close = state.clone();
    let state_for_delete = state.clone();
    let state_for_load = state.clone();
    let state_for_resume = state.clone();
    let state_for_fork = state.clone();
    let negotiation_for_load = negotiation.clone();
    let negotiation_for_resume = negotiation.clone();
    let negotiation_for_fork = negotiation.clone();
    let negotiation_for_compaction = negotiation.clone();
    let negotiation_for_prompt = negotiation.clone();
    let negotiation_for_close = negotiation.clone();
    // The connection's lease-owner id: turns acquire it for their
    // lifetime; released when the turn responds.
    let owner = Arc::new(format!("stdio-{}", uuid::Uuid::new_v4()));
    let owner_for_prompt = owner.clone();
    let state_for_prompt = state.clone();

    let builder = Agent
        .builder()
        .name("tackle")
        .on_receive_request(
            async move |request: InitializeRequest, responder, _cx| {
                // Protocol version negotiation: tackle implements v1, so it
                // responds with v1 regardless of the client's latest; a
                // v1-only client proceeds, a v2-only client disconnects.
                negotiation_for_init.capture(&request.client_capabilities);
                // Missing provider credentials surface as authMethods:
                // the client drives the ACP authenticate method.
                let mut response = InitializeResponse::new(ProtocolVersion::V1)
                    .agent_capabilities(state_for_init.capabilities());
                if first_run::missing_credentials(&state_for_init) {
                    response.auth_methods =
                        vec![agent_client_protocol::schema::v1::AuthMethod::Agent(
                            agent_client_protocol::schema::v1::AuthMethodAgent::new(
                                "credentials",
                                "Set up model credentials",
                            )
                            .description(Some(
                                "Resolve the configured model endpoints' credentials.".to_owned(),
                            )),
                        )];
                }
                responder.respond(response)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: agent_client_protocol::schema::v1::AuthenticateRequest,
                        responder,
                        _cx| {
                // Re-attempt the credential resolution; the next prompt
                // uses whatever resolved.
                let _ = first_run::missing_credentials(&state_for_auth);
                responder.respond(agent_client_protocol::schema::v1::AuthenticateResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: NewSessionRequest, responder, cx| {
                // Default actor selection: [defaults].actor, else the
                // built-in "default" (T-031 adds the client selector).
                let actor = state_for_new
                    .config
                    .config
                    .defaults
                    .actor
                    .clone()
                    .unwrap_or_else(|| "default".into());
                let metadata = serde_json::json!({ "actor": actor }).to_string();
                let session = state_for_new
                    .db
                    .create_session(
                        crate::store::SessionKind::Interactive,
                        &request.cwd.to_string_lossy(),
                        None,
                        None,
                        &metadata,
                    )
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                // MCP connections kick off asynchronously (T-018); session
                // creation never blocks on them. Client-forwarded servers
                // (session/new's mcp_servers) register and connect after
                // the response, below.
                // The first-session seed: harness-authored static content
                // documenting the loaded configuration and the command
                // syntaxes, on its own seed turn.
                let seed = first_run::first_run_seed(&state_for_new);
                let seed_turn = state_for_new
                    .db
                    .append_turn(
                        &session.id,
                        None,
                        crate::store::TurnKind::Seed,
                        None,
                        crate::store::TurnUsage::default(),
                    )
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                state_for_new
                    .db
                    .append_message(
                        &seed_turn.id,
                        crate::store::Role::User,
                        &serde_json::json!([{ "type": "text", "text": seed }])
                            .to_string(),
                        None,
                        None,
                        None,
                        None,
                    )
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                // The selectors ride the response; degraded discovery
                // surfaces as notices.
                let mut selectors = config_options::build(&state_for_new, &session).await;
                for notice in &selectors.notices {
                    if negotiation_for_new.supports(UnstableFeature::SessionNotices) {
                        let _ = cx.send_notification(SessionNotification::new(
                            wire_session_id(&session.id),
                            SessionUpdate::Notice(notice.clone()),
                        ));
                    }
                }
                // Seed the effective model into session metadata when
                // unset: display and the first prompt agree without an
                // explicit client switch.
                if crate::store::session_model_free(&session).is_none() {
                    if let Some(model) = config_options::resolve_model_for(
                        &state_for_new,
                        &session,
                        selectors.first_model.as_deref(),
                    ) {
                        state_for_new
                            .db
                            .set_model(&session.id, &model)
                            .await
                            .map_err(|err| Error::internal_error().data(err.to_string()))?;
                        // Refresh so the response's current matches the
                        // seeded metadata (the chain is pure; rebuild
                        // picks the same effective model).
                        if let Ok(Some(fresh)) = state_for_new.db.get_session(&session.id).await {
                            selectors = config_options::build(&state_for_new, &fresh).await;
                        }
                    }
                }
                let wire_id = wire_session_id(&session.id);
                let response =
                    NewSessionResponse::new(wire_id.clone()).config_options(selectors.options);
                // Zed drops session updates that arrive before the
                // session/new response registers the session
                // (zed-industries/zed#60199) — advertise after.
                responder.respond(response)?;
                // Client-forwarded servers register and connect after the
                // response, so session/new returns immediately while their
                // tools are in the advertisement that follows and their
                // status is known before the first turn.
                connect_forwarded_servers(&negotiation_for_new, &request.mcp_servers).await;
                if let Some(notice) = mcp_status_notice(&negotiation_for_new).await {
                    if negotiation_for_new.supports(UnstableFeature::SessionNotices) {
                        let _ = cx.send_notification(SessionNotification::new(
                            wire_id.clone(),
                            SessionUpdate::Notice(notice),
                        ));
                    }
                }
                let commands = available_commands_update(&state_for_new, &negotiation_for_new).await;
                cx.send_notification(SessionNotification::new(wire_id, commands))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::agent_client_protocol::schema::v1::ListSessionsRequest,
                        responder,
                        _cx| {
                let cwd = request
                    .cwd
                    .as_ref()
                    .map(|cwd| cwd.to_string_lossy().to_string());
                let filter = crate::store::ListFilter {
                    cwd: cwd.as_deref(),
                    include_ephemeral: state_for_list
                        .config
                        .config
                        .sessions
                        .include_ephemeral
                        .unwrap_or(false),
                    include_deleted: false,
                };
                let sessions = state_for_list
                    .db
                    .list_sessions(&filter)
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                let infos: Vec<SessionInfo> = sessions
                    .into_iter()
                    .map(|session| {
                        let mut info =
                            SessionInfo::new(wire_session_id(&session.id), session.cwd.clone())
                                .updated_at(timestamp(session.updated_at));
                        if !session.title.is_empty() {
                            info = info.title(session.title.clone());
                        }
                        if let Some(owner) = &session.owner {
                            let mut meta = agent_client_protocol::schema::v1::Meta::new();
                            meta.insert("owner".into(), owner.clone().into());
                            info = info.meta(meta);
                        }
                        info
                    })
                    .collect();
                responder.respond(ListSessionsResponse::new(infos))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: agent_client_protocol::schema::v1::CloseSessionRequest,
                        responder,
                        cx| {
                state_for_close
                    .db
                    .close_session(&store_session_id(&request.session_id))
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                // The session's active life is over: its permission
                // grants go with it (they never outlive their session).
                state_for_close
                    .grants
                    .lock()
                    .await
                    .clear_session(&request.session_id.to_string());
                // Release the terminals the harness created for this
                // session (Q8 lifecycle): connection-scoped and
                // ephemeral, so best-effort and silent on failure.
                let terminals: Vec<String> = {
                    let mut map = negotiation_for_close
                        .terminals
                        .lock()
                        .expect("negotiation poisoned");
                    map.remove(&request.session_id.to_string())
                        .unwrap_or_default()
                        .into_iter()
                        .collect()
                };
                for terminal_id in terminals {
                    let release = agent_client_protocol::schema::v1::ReleaseTerminalRequest::new(
                        request.session_id.clone(),
                        agent_client_protocol::schema::v1::TerminalId::new(terminal_id),
                    );
                    let _ = cx.send_request(release).block_task().await;
                }
                responder.respond(CloseSessionResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: agent_client_protocol::schema::v1::PromptRequest,
                        responder,
                        cx| {
                use agent_client_protocol::schema::v1::{
                    ContentBlock, PromptResponse, StopReason, UsageUpdate,
                };

                let session_id = store_session_id(&request.session_id);

                // The lease is the one-turn-at-a-time invariant: prompt
                // against an actively-owned session fails precisely.
                if !state_for_prompt
                    .db
                    .acquire_lease(&session_id, &owner_for_prompt, 60)
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?
                {
                    let holder = state_for_prompt
                        .db
                        .lease_holder(&session_id)
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| "another connection".into());
                    return responder.respond_with_error(Error::internal_error().data(
                        serde_json::json!({
                            "error": "session-actively-owned",
                            "owner": holder,
                        }),
                    ));
                }

                // Persist the prompt turn + the user message (original
                // content blocks, stored as the JSON block array).
                let blocks_json =
                    serde_json::to_string(&request.prompt).unwrap_or_else(|_| "[]".to_owned());
                let head = state_for_prompt
                    .db
                    .get_session(&session_id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|session| session.head_turn_id);
                let turn = state_for_prompt
                    .db
                    .append_turn(
                        &session_id,
                        head.as_ref(),
                        crate::store::TurnKind::Interaction,
                        None,
                        crate::store::TurnUsage::default(),
                    )
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                state_for_prompt
                    .db
                    .append_message(
                        &turn.id,
                        crate::store::Role::User,
                        &blocks_json,
                        None,
                        None,
                        None,
                        None,
                    )
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;

                // The /fork fallback for v1 clients: exact first-token
                // match on user-typed input. Creates the fork, seeds it,
                // fires the session_forked scripts, and reports the new
                // session id in the parent turn — no model request.
                let first_text = request
                    .prompt
                    .first()
                    .and_then(|block| match block {
                        ContentBlock::Text(text) => Some(text.text.trim().to_owned()),
                        _ => None,
                    })
                    .unwrap_or_default();
                if first_text.split_whitespace().next() == Some("/fork") {
                    let access = fork_access(&state_for_prompt);
                    let fork_result = fork::run_fork(
                        &state_for_prompt,
                        std::sync::Arc::clone(&access),
                        &session_id,
                    )
                    .await;
                    let fork = match fork_result {
                        Ok(fork) => fork,
                        Err(err) => {
                            state_for_prompt
                                .db
                                .release_lease(&session_id, &owner_for_prompt)
                                .await
                                .ok();
                            return responder
                                .respond_with_error(Error::internal_error().data(err.to_string()));
                        }
                    };
                    // Seeds are harness-authored static user-facing
                    // content with their own messageId.
                    let _ =
                        fork::insert_fork_seed(&state_for_prompt.db, &fork.id, &session_id).await;

                    // The parent-turn report: an agent message carrying
                    // the new session id (wire form for the client).
                    let wire_fork_id = wire_session_id(&fork.id).to_string();
                    let report =
                        fork::fork_report_message(&wire_fork_id, fork.fork_point_turn_id.as_ref());
                    let message = state_for_prompt
                        .db
                        .append_message(
                            &turn.id,
                            crate::store::Role::Assistant,
                            &serde_json::json!([{ "type": "text", "text": report.clone() }])
                                .to_string(),
                            None,
                            None,
                            None,
                            None,
                        )
                        .await
                        .map_err(|err| Error::internal_error().data(err.to_string()))?;
                    let chunk =
                        agent_client_protocol::schema::v1::ContentChunk::new(ContentBlock::Text(
                            agent_client_protocol::schema::v1::TextContent::new(report.clone()),
                        ))
                        .message_id(
                            agent_client_protocol::schema::v1::MessageId::new(message.id.clone()),
                        );
                    let _ = cx.send_notification(SessionNotification::new(
                        request.session_id.clone(),
                        agent_client_protocol::schema::v1::SessionUpdate::AgentMessageChunk(chunk),
                    ));

                    // updatedAt each turn — the fork's report turn too.
                    let updated = agent_client_protocol::schema::v1::SessionInfoUpdate::new()
                        .updated_at(
                            state_for_prompt
                                .db
                                .get_session(&session_id)
                                .await
                                .ok()
                                .flatten()
                                .map(|session| timestamp(session.updated_at))
                                .unwrap_or_default(),
                        );
                    let _ = cx.send_notification(SessionNotification::new(
                        request.session_id.clone(),
                        SessionUpdate::SessionInfoUpdate(updated),
                    ));
                    state_for_prompt
                        .db
                        .release_lease(&session_id, &owner_for_prompt)
                        .await
                        .ok();
                    return responder.respond(PromptResponse::new(StopReason::EndTurn));
                }

                // `/!mcp.<server>.<tool> {json}` or
                // `/!client.<surface>.<method> {json}`: the user calls a
                // tool directly. No model request, and no permission
                // prompt — the call is the user's own keystroke, so the
                // pipeline has nothing to ask. The result is reported as
                // a tool call and stored in the turn.
                if let Some(input) = crate::invokables::parse_direct(&first_text) {
                    let call_id = format!("direct-{}", uuid::Uuid::new_v4());
                    let announce = agent_client_protocol::schema::v1::ToolCall::new(
                        agent_client_protocol::schema::v1::ToolCallId::new(call_id.clone()),
                        input.invokable.clone(),
                    )
                    .status(agent_client_protocol::schema::v1::ToolCallStatus::InProgress)
                    .raw_input(serde_json::from_str(&input.arguments).ok());
                    let _ = cx.send_notification(SessionNotification::new(
                        request.session_id.clone(),
                        agent_client_protocol::schema::v1::SessionUpdate::ToolCall(announce),
                    ));

                    // Direct invocation may issue a request back to the
                    // client (a client-capability tool), and awaiting it
                    // inside the dispatch callback would deadlock the
                    // dispatch loop on its own response. Run the whole
                    // effect on a spawned task, like a model turn, and
                    // respond when it completes.
                    let cx = cx.clone();
                    let state = state_for_prompt.clone();
                    let negotiation = negotiation_for_prompt.clone();
                    let wire_id = request.session_id.clone();
                    let turn_id = turn.id.clone();
                    let owner = owner_for_prompt.clone();
                    let spawn_cx = cx.clone();
                    spawn_cx.spawn(async move {
                        let (text, is_error) = if input.invokable.starts_with("client.") {
                            // Client-capability tools execute on the
                            // connection, not the MCP pool.
                            let available = crate::acp::client_tools::catalog(
                                negotiation.client_caps(),
                                negotiation.editor_toggles(),
                            );
                            match input.arguments_json() {
                                Err(err) => (err.to_string(), true),
                                Ok(_) if !available
                                    .iter()
                                    .any(|tool| tool.name == input.invokable) =>
                                {
                                    (
                                        format!(
                                            "client tool `{}` is not available on this connection",
                                            input.invokable
                                        ),
                                        true,
                                    )
                                }
                                Ok(arguments) => {
                                    match crate::acp::client_tools::execute(
                                        &cx,
                                        &wire_id,
                                        &input.invokable,
                                        &arguments,
                                    )
                                    .await
                                    {
                                        Ok(executed) => {
                                            negotiation.track_terminals(
                                                &wire_id.to_string(),
                                                executed.created,
                                                executed.finished,
                                            );
                                            (executed.content, false)
                                        }
                                        Err(err) => (err, true),
                                    }
                                }
                            }
                        } else {
                            let pool = negotiation.mcp_pool();
                            let pool = pool.lock().await;
                            let tools = pool.list_tools().await;
                            let executed = match crate::invokables::Registry::build(
                                &state.definitions,
                                &tools,
                            ) {
                                Ok(registry) => registry
                                    .execute_direct(&pool, &input)
                                    .await
                                    .map_err(|err| err.to_string()),
                                Err(err) => Err(err.to_string()),
                            };
                            drop(pool);
                            match executed {
                                Ok(result) => (result.content_json, result.is_error),
                                Err(err) => (err, true),
                            }
                        };
                        let status = if is_error {
                            agent_client_protocol::schema::v1::ToolCallStatus::Failed
                        } else {
                            agent_client_protocol::schema::v1::ToolCallStatus::Completed
                        };
                        let _ = cx.send_notification(SessionNotification::new(
                            wire_id.clone(),
                            SessionUpdate::ToolCallUpdate(
                                agent_client_protocol::schema::v1::ToolCallUpdate::new(
                                    agent_client_protocol::schema::v1::ToolCallId::new(call_id),
                                    agent_client_protocol::schema::v1::ToolCallUpdateFields::new()
                                        .status(status)
                                        .content(vec![
                                            agent_client_protocol::schema::v1::ToolCallContent::from(
                                                ContentBlock::Text(
                                                    agent_client_protocol::schema::v1::TextContent::new(
                                                        text.clone(),
                                                    ),
                                                ),
                                            ),
                                        ]),
                                ),
                            ),
                        ));
                        // Two stored messages per the schema: the call
                        // (namespaced invokable plus arguments) and the
                        // result that answers it.
                        let call = state
                            .db
                            .append_message(
                                &turn_id,
                                crate::store::Role::ToolCall,
                                &input.arguments,
                                Some(&input.invokable),
                                None,
                                None,
                                None,
                            )
                            .await
                            .map_err(|err| Error::internal_error().data(err.to_string()))?;
                        state
                            .db
                            .append_message(
                                &turn_id,
                                crate::store::Role::ToolResult,
                                &serde_json::json!([{ "type": "text", "text": text }]).to_string(),
                                Some(&input.invokable),
                                Some(&call.id),
                                Some(is_error),
                                None,
                            )
                            .await
                            .map_err(|err| Error::internal_error().data(err.to_string()))?;
                        state
                            .db
                            .release_lease(&session_id, &owner)
                            .await
                            .ok();
                        responder.respond(PromptResponse::new(StopReason::EndTurn))
                    })?;
                    return Ok(());
                }

                // Runtime availability commands: `/mcp` and `/editor`
                // are handled before any model request. An unrecognised
                // subcommand yields no reply and falls through to
                // normal processing.
                if first_text == "/mcp"
                    || first_text.starts_with("/mcp ")
                    || first_text == "/editor"
                    || first_text.starts_with("/editor ")
                {
                    let reply = if first_text.starts_with("/editor") {
                        editor_command(&negotiation_for_prompt, &first_text)
                    } else {
                        mcp_command(&negotiation_for_prompt, &first_text).await
                    };
                    if let Some(reply) = reply {
                        let message = state_for_prompt
                            .db
                            .append_message(
                                &turn.id,
                                crate::store::Role::Assistant,
                                &serde_json::json!([{ "type": "text", "text": reply }])
                                    .to_string(),
                                None,
                                None,
                                None,
                                None,
                            )
                            .await
                            .map_err(|err| Error::internal_error().data(err.to_string()))?;
                        let chunk =
                            agent_client_protocol::schema::v1::ContentChunk::new(ContentBlock::Text(
                                agent_client_protocol::schema::v1::TextContent::new(reply),
                            ))
                            .message_id(agent_client_protocol::schema::v1::MessageId::new(
                                message.id.clone(),
                            ));
                        let _ = cx.send_notification(SessionNotification::new(
                            request.session_id.clone(),
                            agent_client_protocol::schema::v1::SessionUpdate::AgentMessageChunk(chunk),
                        ));
                        // The tool surface changed shape, so the command
                        // listing is republished for the session.
                        let commands = available_commands_update(&state_for_prompt, &negotiation_for_prompt).await;
                        let _ = cx.send_notification(SessionNotification::new(
                            request.session_id.clone(),
                            commands,
                        ));
                        state_for_prompt
                            .db
                            .release_lease(&session_id, &owner_for_prompt)
                            .await
                            .ok();
                        return responder.respond(PromptResponse::new(StopReason::EndTurn));
                    }
                }

                // Resolve the actor for this session (compaction's
                // event payload and the model turn both need it).
                let session_row = state_for_prompt
                    .db
                    .get_session(&session_id)
                    .await
                    .ok()
                    .flatten();
                let actor_name = session_row
                    .as_ref()
                    .map(|row| {
                        session_actor(
                            row,
                            state_for_prompt.config.config.defaults.actor.as_deref(),
                        )
                    })
                    .unwrap_or_else(|| "default".into());

                // Compaction-tagged prompts: intercepted — the
                // compaction_requested script event fires instead of a
                // model turn in the main session.
                if compaction::is_compaction_command(&state_for_prompt.definitions, &first_text) {
                    let size = session_row
                        .as_ref()
                        .and_then(|row| {
                            config_options::resolve_model_for(&state_for_prompt, row, None)
                        })
                        .and_then(|model_ref| context_window(&model_ref))
                        .unwrap_or(0);
                    let (before, after) = compaction::run_compaction(
                        &state_for_prompt,
                        &session_id,
                        &turn.id,
                        &actor_name,
                        size,
                    )
                    .await;

                    // The in-band announcement: trigger and counts.
                    let note =
                        compaction::announcement(first_text.trim_start_matches('/'), before, after);
                    let message = state_for_prompt
                        .db
                        .append_message(
                            &turn.id,
                            crate::store::Role::Assistant,
                            &serde_json::json!([{ "type": "text", "text": note.clone() }])
                                .to_string(),
                            None,
                            None,
                            None,
                            None,
                        )
                        .await
                        .map_err(|err| Error::internal_error().data(err.to_string()))?;
                    let chunk =
                        agent_client_protocol::schema::v1::ContentChunk::new(ContentBlock::Text(
                            agent_client_protocol::schema::v1::TextContent::new(note.clone()),
                        ))
                        .message_id(
                            agent_client_protocol::schema::v1::MessageId::new(message.id.clone()),
                        );
                    let _ = cx.send_notification(SessionNotification::new(
                        request.session_id.clone(),
                        agent_client_protocol::schema::v1::SessionUpdate::AgentMessageChunk(chunk),
                    ));

                    // The visible usage_update drop.
                    if size > 0 {
                        let _ = cx.send_notification(SessionNotification::new(
                            request.session_id.clone(),
                            SessionUpdate::UsageUpdate(UsageUpdate::new(after, size)),
                        ));
                    }

                    // RFD-shaped compaction updates flow only to
                    // clients advertising the capability.
                    if negotiation_for_compaction.supports(UnstableFeature::SessionCompaction) {
                        let update = agent_client_protocol::schema::v1::CompactionUpdate::new(
                            agent_client_protocol::schema::v1::CompactionId::new(
                                uuid::Uuid::new_v4().to_string(),
                            ),
                            agent_client_protocol::schema::v1::CompactionStatus::Completed,
                        )
                        .summary(vec![
                            agent_client_protocol::schema::v1::ContentBlock::Text(
                                agent_client_protocol::schema::v1::TextContent::new(note.clone()),
                            ),
                        ]);
                        let _ = cx.send_notification(SessionNotification::new(
                            request.session_id.clone(),
                            SessionUpdate::CompactionUpdate(update),
                        ));
                    }

                    // updatedAt each turn — intercepted turns included.
                    let updated = agent_client_protocol::schema::v1::SessionInfoUpdate::new()
                        .updated_at(
                            state_for_prompt
                                .db
                                .get_session(&session_id)
                                .await
                                .ok()
                                .flatten()
                                .map(|session| timestamp(session.updated_at))
                                .unwrap_or_default(),
                        );
                    let _ = cx.send_notification(SessionNotification::new(
                        request.session_id.clone(),
                        SessionUpdate::SessionInfoUpdate(updated),
                    ));

                    state_for_prompt
                        .db
                        .release_lease(&session_id, &owner_for_prompt)
                        .await
                        .ok();
                    return responder.respond(PromptResponse::new(StopReason::EndTurn));
                }

                // The model turn is long and awaits client messages
                // (request_permission); the connection processes messages
                // on one task, so the turn runs spawned, keeping the loop
                // free to route the client's replies.
                let cx = cx.clone();
                let state_for_prompt = state_for_prompt.clone();
                let owner_for_prompt = owner_for_prompt.clone();
                let negotiation_for_turn = negotiation_for_prompt.clone();
                let spawn_cx = cx.clone();
                spawn_cx.spawn(async move {
                let actor = state_for_prompt.definitions.actors.get(&actor_name);
                let persona = actor.and_then(|a| {
                    state_for_prompt
                        .definitions
                        .personas
                        .get(&a.frontmatter.persona)
                });
                let mcp_tokens = {
                    let pool = negotiation_for_turn.mcp_pool();
                    let pool = pool.lock().await;
                    pool.list_tools().await
                };
                // The client-capability tools the connection advertised
                // and the categories currently enabled (config seeded,
                // `/editor` overridable).
                let client_tools = crate::acp::client_tools::catalog(
                    negotiation_for_turn.client_caps(),
                    negotiation_for_turn.editor_toggles(),
                );
                let registry = crate::invokables::Registry::build_with_client_tools(
                    &state_for_prompt.definitions,
                    &mcp_tokens,
                    &client_tools,
                )
                .unwrap_or_default();
                let tools = registry.model_tools();
                // FR-012: the invokable listing rides the system prompt.
                let mut system = persona.map(|p| p.body.clone()).unwrap_or_default();
                if !tools.is_empty() {
                    system.push_str("\n\n");
                    system.push_str(&registry.manifest());
                }
                // The model: the shared resolution chain — metadata,
                // actor frontmatter, [defaults].model, then the first
                // listed selector option. Nothing left is a clear
                // configuration error, never a `default/model` sentinel.
                let session_for_model = session_row.as_ref().ok_or_else(|| {
                    Error::internal_error().data("no such session".to_owned())
                })?;
                let model_ref = {
                    if let Some(model_ref) =
                        config_options::resolve_model_for(&state_for_prompt, session_for_model, None)
                    {
                        model_ref
                    } else {
                        let listed =
                            config_options::build(&state_for_prompt, session_for_model).await;
                        listed.first_model.ok_or_else(|| {
                            Error::internal_error().data(
                                "no model configured; set [defaults].model or select a model"
                                    .to_owned(),
                            )
                        })?
                    }
                };
                let size = context_window(&model_ref).unwrap_or(0);

                // Assemble the prior context (the prompt turn included —
                // its user message is the prompt; the loop re-sends it as
                // the final message).
                let assembled = state_for_prompt
                    .db
                    .assemble_context(&session_id)
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                let prior: Vec<crate::agent::ChatMessage> =
                    compaction::prior_messages(&assembled, &turn.id);
                // FR-010: a user-typed `/name` prompt expands into the
                // model prompt (the stored message stays literal).
                let prompt_text = match first_text.strip_prefix('/') {
                    Some(rest) => {
                        let mut parts = rest.splitn(2, char::is_whitespace);
                        let name = parts.next().unwrap_or("");
                        let args = parts.next().unwrap_or("").trim();
                        registry
                            .expand_user(name, args)
                            .unwrap_or_else(|_| prompt_text_of(&blocks_json))
                    }
                    None => prompt_text_of(&blocks_json),
                };

                let provider = crate::agent::provider::provider_for(
                    &model_ref,
                    &state_for_prompt.config.config,
                )
                .map_err(|err| Error::internal_error().data(err.to_string()))?;
                // The context-length failure note: actionable, naming
                // the configured compaction script (or its absence).
                let context_length_note = compaction::compaction_disabled_note(
                    state_for_prompt
                        .config
                        .config
                        .scripts
                        .iter()
                        .find(|(_, script)| {
                            script.enabled
                                && script
                                    .events
                                    .iter()
                                    .any(|event| event == "compaction_requested")
                        })
                        .map(|(name, _)| name.as_str()),
                );
                let assistant_id = uuid::Uuid::new_v4().to_string();
                // The turn span: parent of the tool-call and usage
                // spans the loop's paths create.
                let turn_span =
                    crate::telemetry::turn_span(&session_id, &actor_name, &model_ref);
                let _turn_guard = turn_span.enter();
                let session_for_chunks = request.session_id.clone();
                let cx_for_chunks = cx.clone();
                let cx_for_retry = cx.clone();
                let session_for_retry = request.session_id.clone();

                let outcome = crate::agent::turn::run_turn(
                    &provider,
                    &state_for_prompt.db,
                    &session_id,
                    &turn.id,
                    &assistant_id.to_string(),
                    &model_ref,
                    &system,
                    &prompt_text,
                    prior,
                    tools,
                    state_for_prompt
                        .config
                        .config
                        .turns
                        .max_model_requests
                        .unwrap_or(8),
                    size,
                    &context_length_note,
                    // usage cadence: a usage_update per model request,
                    // skipped when the window is unknown (RFD guidance).
                    {
                        let cx = cx_for_chunks.clone();
                        let session_for_usage = session_for_chunks.clone();
                        move |input: u64, _output: u64, _cost: f64| {
                            if size == 0 {
                                return;
                            }
                            let _ = cx.send_notification(SessionNotification::new(
                                session_for_usage.clone(),
                                SessionUpdate::UsageUpdate(UsageUpdate::new(input, size)),
                            ));
                        }
                    },
                    move |delta: &str| {
                        let chunk = agent_client_protocol::schema::v1::ContentChunk::new(
                            ContentBlock::Text(
                                agent_client_protocol::schema::v1::TextContent::new(
                                    delta.to_owned(),
                                ),
                            ),
                        )
                        .message_id(
                            agent_client_protocol::schema::v1::MessageId::new(assistant_id.clone()),
                        );
                        let _ = cx_for_chunks.send_notification(SessionNotification::new(
                            session_for_chunks.clone(),
                            agent_client_protocol::schema::v1::SessionUpdate::AgentMessageChunk(
                                chunk,
                            ),
                        ));
                    },
                    // retry attempts surface as tool-call status cards
                    // (the stable ACP surface's only card mechanism).
                    {
                        let cx = cx_for_retry;
                        move |attempt: u32, message: &str| {
                            let card = agent_client_protocol::schema::v1::ToolCall::new(
                                agent_client_protocol::schema::v1::ToolCallId::new(format!(
                                    "retry-{attempt}-{}",
                                    uuid::Uuid::new_v4()
                                )),
                                format!("Retrying model request (attempt {attempt}): {message}"),
                            )
                            .status(agent_client_protocol::schema::v1::ToolCallStatus::InProgress);
                            let _ = cx.send_notification(SessionNotification::new(
                                session_for_retry.clone(),
                                agent_client_protocol::schema::v1::SessionUpdate::ToolCall(card),
                            ));
                        }
                    },
                    // Model tool calls: announce, run the permission
                    // pipeline, execute via the registry, report the result
                    // back into the loop.
                    {
                        let registry = registry.clone();
                        let state = state_for_prompt.clone();
                        let negotiation = negotiation_for_turn.clone();
                        let cx = cx.clone();
                        let session = request.session_id.clone();
                        let actor = actor_name.clone();
                        let sources = policy_script_sources(&state_for_prompt);
                        let client_tools = client_tools.clone();
                        move |call: crate::agent::ModelToolCall| -> std::pin::Pin<
                            Box<
                                dyn std::future::Future<
                                        Output = crate::agent::ToolCallOutcome,
                                    > + Send,
                            >,
                        > {
                            let registry = registry.clone();
                            let state = state.clone();
                            let negotiation = negotiation.clone();
                            let cx = cx.clone();
                            let session = session.clone();
                            let actor = actor.clone();
                            let sources = sources.clone();
                            let client_tools = client_tools.clone();
                            Box::pin(async move {
                                run_model_tool_call(
                                    ToolCallContext {
                                        registry: &registry,
                                        state: &state,
                                        negotiation: &negotiation,
                                        cx: &cx,
                                        session_id: &session,
                                        actor: &actor,
                                        policy_sources: &sources,
                                        client_tools: &client_tools,
                                    },
                                    call,
                                )
                                .await
                            })
                        }
                    },
                )
                .await;

                if let Ok(result) = &outcome {
                    let _usage = crate::telemetry::usage_span(
                        &session_id,
                        result.usage.input_tokens,
                        result.usage.output_tokens,
                    );
                }

                // updatedAt each turn: the client-visible last-activity
                // stamp, before the response.
                let updated = agent_client_protocol::schema::v1::SessionInfoUpdate::new()
                    .updated_at(
                        state_for_prompt
                            .db
                            .get_session(&session_id)
                            .await
                            .ok()
                            .flatten()
                            .map(|session| timestamp(session.updated_at))
                            .unwrap_or_default(),
                    );
                let _ = cx.send_notification(SessionNotification::new(
                    request.session_id.clone(),
                    SessionUpdate::SessionInfoUpdate(updated),
                ));

                // Automatic compaction: the post_turn event fires after
                // every model turn, detached so a slow summarisation
                // never blocks the response.
                compaction::fire_post_turn_detached(
                    state_for_prompt.clone(),
                    session_id.clone(),
                    turn.id.clone(),
                    actor_name.clone(),
                    size,
                );

                // Titling: detached, silent, only for untitled sessions.
                titling::fire_title_trigger_detached(
                    state_for_prompt.clone(),
                    session_id.clone(),
                    size,
                );

                state_for_prompt
                    .db
                    .release_lease(&session_id, &owner_for_prompt)
                    .await
                    .ok();

                match outcome {
                    Ok(result) => responder.respond(PromptResponse::new(match result.stop {
                        crate::agent::TurnStop::EndTurn => StopReason::EndTurn,
                        crate::agent::TurnStop::MaxTurnRequests => StopReason::MaxTurnRequests,
                    })),
                    Err(err) => responder.respond_with_error(
                        Error::internal_error()
                            .data(serde_json::json!({ "error": err.to_string() })),
                    ),
                }
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: agent_client_protocol::schema::v1::DeleteSessionRequest,
                        responder,
                        _cx| {
                state_for_delete
                    .db
                    .delete_session(&store_session_id(&request.session_id))
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                state_for_delete
                    .grants
                    .lock()
                    .await
                    .clear_session(&request.session_id.to_string());
                responder.respond(DeleteSessionResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: LoadSessionRequest, responder, cx| {
                // A forwarded server's tools must be addressable before
                // the re-advertisement below lists them.
                connect_forwarded_servers(&negotiation_for_load, &request.mcp_servers).await;
                // Replay: full user-visible history in DAG order. Usage
                // snapshot emission lands with T-016's model metadata.
                let assembled = state_for_load
                    .db
                    .assemble_context(&store_session_id(&request.session_id))
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                for replay in replay_updates(&request.session_id, &assembled) {
                    cx.send_notification(replay)
                        .map_err(|err| Error::internal_error().data(err.to_string()))?;
                }
                // Re-advertise on load: older Zed builds rebuild the
                // thread UI from retained state and treat commands as
                // transient (zed-industries/zed#53209), and the
                // pre-response window drops updates outright. Selectors
                // ride the response so a reopened session can switch
                // models before the next prompt.
                let session = state_for_load
                    .db
                    .get_session(&store_session_id(&request.session_id))
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?
                    .ok_or_else(|| Error::internal_error().data("no such session".to_owned()))?;
                let selectors = config_options::build(&state_for_load, &session).await;
                let commands = available_commands_update(&state_for_load, &negotiation_for_load).await;
                // The usage snapshot: exactly one after replay, so a
                // reopened client shows the session's context state
                // (FR-005, FR-007).
                let size = config_options::resolve_model_for(&state_for_load, &session, None)
                    .and_then(|model_ref| context_window(&model_ref))
                    .unwrap_or(0);
                if size > 0 {
                    let used_bytes: u64 = assembled
                        .iter()
                        .map(|item| item.message.content.len() as u64)
                        .sum();
                    let used = crate::scripts::host::estimate_tokens(used_bytes);
                    cx.send_notification(SessionNotification::new(
                        request.session_id.clone(),
                        SessionUpdate::UsageUpdate(
                            agent_client_protocol::schema::v1::UsageUpdate::new(used, size),
                        ),
                    ))
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                }
                responder.respond(
                    LoadSessionResponse::new().config_options(selectors.options),
                )?;
                for notice in &selectors.notices {
                    if negotiation_for_load.supports(UnstableFeature::SessionNotices) {
                        let _ = cx.send_notification(SessionNotification::new(
                            request.session_id.clone(),
                            SessionUpdate::Notice(notice.clone()),
                        ));
                    }
                }
                cx.send_notification(SessionNotification::new(
                    request.session_id.clone(),
                    commands,
                ))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: ResumeSessionRequest, responder, cx| {
                // Resume reattaches WITHOUT replay (FR-003); config
                // selectors ride the response like session/new.
                connect_forwarded_servers(&negotiation_for_resume, &request.mcp_servers).await;
                let session = state_for_resume
                    .db
                    .get_session(&store_session_id(&request.session_id))
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?
                    .ok_or_else(|| Error::internal_error().data("no such session".to_owned()))?;
                let selectors = config_options::build(&state_for_resume, &session).await;
                responder.respond(
                    ResumeSessionResponse::new().config_options(selectors.options),
                )?;
                for notice in &selectors.notices {
                    if negotiation_for_resume.supports(UnstableFeature::SessionNotices) {
                        let _ = cx.send_notification(SessionNotification::new(
                            request.session_id.clone(),
                            SessionUpdate::Notice(notice.clone()),
                        ));
                    }
                }
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: agent_client_protocol::schema::v1::SetSessionConfigOptionRequest,
                        responder,
                        cx| {
                use agent_client_protocol::schema::v1::SessionConfigOptionValue;
                let session_id = store_session_id(&request.session_id);
                let session = state_for_config
                    .db
                    .get_session(&session_id)
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?
                    .ok_or_else(|| {
                        Error::internal_error().data("no such session".to_owned())
                    })?;

                let applied = match &request.value {
                    SessionConfigOptionValue::ValueId { value } => {
                        let value = value.to_string();
                        match request.config_id.0.as_ref() {
                            config_options::MODEL_SELECTOR_ID => {
                                let access: std::sync::Arc<dyn fork::SessionAccess> =
                                    std::sync::Arc::new(compaction::StoreAccess::new(
                                        std::sync::Arc::clone(&state_for_config.db),
                                        std::sync::Arc::new(compaction::LoopPendingDispatcher),
                                        std::sync::Arc::new(compaction::LoopPendingCompletions),
                                        0,
                                    ));
                                let mut updates: Vec<agent_client_protocol::schema::v1::Notice> =
                                    Vec::new();
                                let mut notify =
                                    |update: config_options::SessionUpdateForOptions| {
                                        match update {
                                            config_options::SessionUpdateForOptions::Notice(
                                                notice,
                                            ) => {
                                                updates.push(notice);
                                            }
                                        }
                                    };
                                let result = config_options::apply_model_switch(
                                    &state_for_config,
                                    access.as_ref(),
                                    &session_id,
                                    &value,
                                    &mut notify,
                                )
                                .await;
                                for notice in updates {
                                    if negotiation_for_config.supports(UnstableFeature::SessionNotices) {
                                        let _ = cx.send_notification(SessionNotification::new(
                                            request.session_id.clone(),
                                            SessionUpdate::Notice(notice),
                                        ));
                                    }
                                }
                                result
                            }
                            config_options::ACTOR_SELECTOR_ID => {
                                config_options::apply_actor_switch(
                                    &state_for_config,
                                    &session_id,
                                    &value,
                                )
                                .await
                            }
                            other => Err(format!("unknown config option `{other}`")),
                        }
                    }
                    SessionConfigOptionValue::Boolean { .. } => {
                        Err("boolean options are not configured".to_owned())
                    }
                    other => Err(format!("unsupported config value: {other:?}")),
                };

                // The refreshed selectors; the switch is effective the
                // following turn.
                let refreshed = config_options::build(&state_for_config, &session).await;
                match applied {
                    Ok(()) => {
                        let _ = cx.send_notification(SessionNotification::new(
                            request.session_id.clone(),
                            SessionUpdate::ConfigOptionUpdate(
                                agent_client_protocol::schema::v1::ConfigOptionUpdate::new(
                                    refreshed.options.clone(),
                                ),
                            ),
                        ));
                        responder.respond(
                            agent_client_protocol::schema::v1::SetSessionConfigOptionResponse::new(
                                refreshed.options,
                            ),
                        )
                    }
                    Err(message) => responder.respond_with_error(
                        Error::internal_error().data(serde_json::json!({ "error": message })),
                    ),
                }
            },
            agent_client_protocol::on_receive_request!(),
        );

    // The RFD path for forking: session/fork, served to clients that
    // call it; the fork capability advertisement is itself gated.
    let builder = {
        builder.on_receive_request(
            async move |request: agent_client_protocol::schema::v1::ForkSessionRequest,
                        responder,
                        cx| {
                // No session updates flow here — the client attaches
                // (session/load) and updates follow.
                let source = store_session_id(&request.session_id);
                let access = fork_access(&state_for_fork);
                let fork = fork::create_fork(&state_for_fork.db, &source)
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                fork::fire_session_forked(
                    &state_for_fork,
                    access,
                    &fork.id,
                    &source,
                    fork.fork_point_turn_id.as_ref(),
                );
                // Selectors on the fork response: the client attaching
                // can switch before the first prompt.
                let selectors = config_options::build(&state_for_fork, &fork).await;
                for notice in &selectors.notices {
                    if negotiation_for_fork.supports(UnstableFeature::SessionNotices) {
                        let _ = cx.send_notification(SessionNotification::new(
                            wire_session_id(&fork.id),
                            SessionUpdate::Notice(notice.clone()),
                        ));
                    }
                }
                responder.respond(
                    agent_client_protocol::schema::v1::ForkSessionResponse::new(wire_session_id(
                        &fork.id,
                    ))
                    .config_options(selectors.options),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
    };

    builder.connect_to(transport).await
}

/// The session access the fork scripts act through: the shared store
/// backend with the loop dispatcher pending (T-015 integration); model
/// turns driven by scripts queue as user messages meanwhile.
fn fork_access(state: &TackleState) -> std::sync::Arc<dyn fork::SessionAccess> {
    struct LoopPendingDispatcher;
    impl fork::PromptDispatcher for LoopPendingDispatcher {
        fn dispatch(
            &self,
            _session: &crate::store::SessionId,
            _text: &str,
        ) -> Result<(), fork::HostError> {
            // The agent loop integration drives these model turns; the
            // user message is already stored, so nothing is lost.
            Ok(())
        }
    }
    struct LoopPendingCompletions;
    impl fork::CompletionSink for LoopPendingCompletions {
        fn publish(&self, _session: &crate::store::SessionId, _completion: fork::Completion) {}
        fn take(&self, _session: &crate::store::SessionId) -> Option<fork::Completion> {
            // The agent loop integration drives these model turns;
            // scripts awaiting a completion degrade gracefully on a
            // non-end_turn answer instead of hanging on their timeout.
            Some(fork::Completion {
                stop_reason: "model-loop-pending".to_owned(),
                final_message: String::new(),
                input_tokens: 0,
                output_tokens: 0,
            })
        }
    }
    std::sync::Arc::new(fork::StoreAccess::new(
        std::sync::Arc::clone(&state.db),
        std::sync::Arc::new(LoopPendingDispatcher),
        std::sync::Arc::new(LoopPendingCompletions),
        0,
    ))
}

/// Maps stored messages to replay notifications: user/assistant messages
/// become message chunks (all blocks, sharing the stored message id —
/// stable identity per logical message); tool call/result pairs collapse
/// into one completed tool-call update; seeds replay as agent chunks;
/// compaction summaries are skipped in stable v1 replay.
fn replay_updates(
    session_id: &SessionId,
    assembled: &[crate::store::AssembledMessage],
) -> Vec<SessionNotification> {
    use agent_client_protocol::schema::v1::{
        ContentBlock, ContentChunk, MessageId, ToolCall, ToolCallStatus,
    };

    let mut updates = Vec::new();
    for assembled in assembled {
        let message = &assembled.message;
        let message_id = MessageId::new(message.id.clone());

        // Tool messages carry raw JSON (arguments/results), not content
        // blocks, so handle them before the block parser.
        match message.role {
            crate::store::Role::ToolCall => {
                // The pair collapses: one completed tool-call update.
                updates.push(notification(
                    session_id,
                    SessionUpdate::ToolCall(
                        ToolCall::new(
                            message.id.clone(),
                            message.tool_name.clone().unwrap_or_default(),
                        )
                        .status(ToolCallStatus::Completed),
                    ),
                ));
                continue;
            }
            crate::store::Role::ToolResult => continue,
            _ => {}
        }

        let blocks: Vec<ContentBlock> = match serde_json::from_str(&message.content) {
            Ok(blocks) => blocks,
            Err(_) => continue, // unparseable stored content: skip, never crash replay
        };

        match message.role {
            crate::store::Role::User => {
                for block in blocks {
                    updates.push(notification(
                        session_id,
                        SessionUpdate::UserMessageChunk(
                            ContentChunk::new(block).message_id(message_id.clone()),
                        ),
                    ));
                }
            }
            crate::store::Role::Assistant | crate::store::Role::System => {
                for block in blocks {
                    updates.push(notification(
                        session_id,
                        SessionUpdate::AgentMessageChunk(
                            ContentChunk::new(block).message_id(message_id.clone()),
                        ),
                    ));
                }
            }
            _ => {}
        }
    }
    updates
}

fn notification(session_id: &SessionId, update: SessionUpdate) -> SessionNotification {
    SessionNotification::new(session_id.clone(), update)
}

/// The registered `pre_tool_use` policy script sources (FR-015).
fn policy_script_sources(state: &TackleState) -> Vec<String> {
    let user_dir = state
        .config
        .user_layer
        .as_deref()
        .and_then(std::path::Path::parent);
    state
        .config
        .config
        .scripts
        .values()
        .filter(|script| {
            script.enabled
                && script
                    .events
                    .iter()
                    .any(|event| event == "pre_tool_use")
        })
        .filter_map(|script| {
            crate::builtins::script_source(&script.file)
                .map(str::to_owned)
                .or_else(|| {
                    user_dir.and_then(|dir| {
                        std::fs::read_to_string(dir.join("scripts").join(&script.file)).ok()
                    })
                })
        })
        .collect()
}

/// The per-call context for a model tool call: the registry, shared
/// state, live connection, session, actor, policy sources, the
/// client-capability tools currently available, and the connection whose
/// terminals the call may create or finish.
struct ToolCallContext<'a> {
    registry: &'a crate::invokables::Registry,
    state: &'a Arc<TackleState>,
    negotiation: &'a ConnectionNegotiation,
    cx: &'a agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    session_id: &'a SessionId,
    actor: &'a str,
    policy_sources: &'a [String],
    client_tools: &'a [crate::acp::client_tools::ClientTool],
}

/// Runs one model tool call: announce it, run the permission pipeline,
/// execute on allow, report the terminal status, and return the result
/// for the next model request (FR-009/014).
async fn run_model_tool_call(
    ctx: ToolCallContext<'_>,
    call: crate::agent::ModelToolCall,
) -> crate::agent::ToolCallOutcome {
    use agent_client_protocol::schema::v1 as v1;
    let ToolCallContext {
        registry,
        state,
        negotiation,
        cx,
        session_id,
        actor,
        policy_sources,
        client_tools,
    } = ctx;
    let arguments: serde_json::Value =
        serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
    let kind = if call.name.starts_with("mcp.") {
        "mcp"
    } else if call.name.starts_with("prompt.") {
        "prompt"
    } else if call.name.starts_with("client.") {
        "client"
    } else {
        "agent"
    };

    let _ = cx.send_notification(SessionNotification::new(
        session_id.clone(),
        SessionUpdate::ToolCall(
            v1::ToolCall::new(v1::ToolCallId::new(call.id.clone()), call.name.clone())
                .status(v1::ToolCallStatus::InProgress)
                .raw_input(Some(arguments.clone())),
        ),
    ));

    let policy = crate::scripts::PolicyScripts::load(policy_sources);
    let requester = AcpPermissionRequester {
        cx: cx.clone(),
        session_id: session_id.clone(),
    };
    let verdict = {
        let mut grants = state.grants.lock().await;
        let assessment = crate::permissions::Assessment {
            session_id: session_id.to_string(),
            actor: actor.to_owned(),
            invokable: call.name.clone(),
            kind,
            arguments: arguments.clone(),
            interactive: true,
        };
        let mut pipeline =
            crate::permissions::PermissionPipeline::new(&mut grants, &policy, &requester);
        pipeline.assess(&assessment).await
    };

    let (content, is_error) = match verdict {
        crate::permissions::Verdict::Allow => {
            if call.name.starts_with("client.") {
                if !client_tools.iter().any(|tool| tool.name == call.name) {
                    (
                        format!(
                            "client tool `{}` is not available on this connection",
                            call.name
                        ),
                        true,
                    )
                } else {
                    match crate::acp::client_tools::execute(cx, session_id, &call.name, &arguments)
                        .await
                    {
                        Ok(executed) => {
                            negotiation.track_terminals(
                                &session_id.to_string(),
                                executed.created,
                                executed.finished,
                            );
                            (executed.content, false)
                        }
                        Err(err) => (err, true),
                    }
                }
            } else if call.name.starts_with("agent.") {
                // FR-026: spawn a nested agent instance of the target
                // actor and return its final message as the tool result.
                match run_agent_invocation(state, session_id, &call.name, &arguments).await {
                    Ok(text) => (text, false),
                    Err(err) => (err, true),
                }
            } else {
                let pool = negotiation.mcp_pool();
                let pool = pool.lock().await;
                match registry.execute_model(&pool, &call.name, &arguments).await {
                    Ok(text) => (text, false),
                    Err(err) => (err, true),
                }
            }
        }
        crate::permissions::Verdict::Deny { reason } => {
            (format!("Permission denied: {reason}"), true)
        }
    };

    let status = if is_error {
        v1::ToolCallStatus::Failed
    } else {
        v1::ToolCallStatus::Completed
    };
    let _ = cx.send_notification(SessionNotification::new(
        session_id.clone(),
        SessionUpdate::ToolCallUpdate(
            v1::ToolCallUpdate::new(
                v1::ToolCallId::new(call.id.clone()),
                v1::ToolCallUpdateFields::new()
                    .status(status)
                    .content(vec![v1::ToolCallContent::from(v1::ContentBlock::Text(
                        v1::TextContent::new(content.clone()),
                    ))]),
            ),
        ),
    ));

    crate::agent::ToolCallOutcome { content, is_error }
}

/// Runs an `agent.<actor>` invocation (FR-026): an ephemeral fork of the
/// parent session runs one nested agent instance of the target actor and
/// its final message becomes the tool result.
async fn run_agent_invocation(
    state: &Arc<TackleState>,
    parent_session: &SessionId,
    invokable: &str,
    arguments: &serde_json::Value,
) -> Result<String, String> {
    let target = invokable.strip_prefix("agent.").unwrap_or(invokable);
    let parent = store_session_id(parent_session);
    let actor = state
        .definitions
        .actors
        .get(target)
        .ok_or_else(|| format!("unknown actor `{target}`"))?;
    let persona = state
        .definitions
        .personas
        .get(&actor.frontmatter.persona)
        .map(|persona| persona.body.clone())
        .unwrap_or_default();
    let prompt = arguments
        .get("prompt")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    if prompt.is_empty() {
        return Err("agent invocations require a `prompt` argument".to_owned());
    }
    let session = state
        .db
        .get_session(&parent)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| "no such session".to_owned())?;
    let model_ref = actor
        .frontmatter
        .model
        .clone()
        .or_else(|| config_options::resolve_model_for(state, &session, None))
        .ok_or_else(|| "no model configured for the nested agent".to_owned())?;
    let provider = crate::agent::provider::provider_for(&model_ref, &state.config.config)
        .map_err(|err| err.to_string())?;
    let result = crate::agent::nested::run_nested_agent(
        &provider,
        &state.db,
        &parent,
        target,
        &persona,
        &model_ref,
        &prompt,
        state.config.config.turns.max_model_requests.unwrap_or(8),
    )
    .await
    .map_err(|err| err.to_string())?;
    Ok(result.final_message)
}

/// The ACP `session/request_permission` bridge (FR-014).
struct AcpPermissionRequester {
    cx: agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    session_id: SessionId,
}

fn permission_option_id(choice: crate::permissions::PermissionChoice) -> &'static str {
    use crate::permissions::PermissionChoice as C;
    match choice {
        C::AllowOnce => "allow_once",
        C::AllowAlways => "allow_always",
        C::RejectOnce => "reject_once",
        C::RejectAlways => "reject_always",
        C::Cancelled => "cancelled",
    }
}

fn permission_option_kind(
    choice: crate::permissions::PermissionChoice,
) -> agent_client_protocol::schema::v1::PermissionOptionKind {
    use agent_client_protocol::schema::v1::PermissionOptionKind as K;
    use crate::permissions::PermissionChoice as C;
    match choice {
        C::AllowOnce => K::AllowOnce,
        C::AllowAlways => K::AllowAlways,
        C::RejectOnce => K::RejectOnce,
        C::RejectAlways => K::RejectAlways,
        C::Cancelled => K::RejectOnce,
    }
}

impl crate::permissions::PermissionRequester for AcpPermissionRequester {
    fn request(
        &self,
        request: crate::permissions::PermissionRequest,
    ) -> impl std::future::Future<Output = crate::permissions::PermissionChoice> + Send {
        use agent_client_protocol::schema::v1 as v1;
        let choices = crate::permissions::ask_options();
        let options: Vec<v1::PermissionOption> = choices
            .iter()
            .map(|(label, choice)| {
                v1::PermissionOption::new(
                    permission_option_id(*choice),
                    (*label).to_owned(),
                    permission_option_kind(*choice),
                )
            })
            .collect();
        let mut fields = v1::ToolCallUpdateFields::new()
            .name(request.invokable.clone())
            .title(request.invokable.clone());
        if let Some(reason) = &request.ask_reason {
            fields = fields.content(vec![v1::ToolCallContent::from(v1::ContentBlock::Text(
                v1::TextContent::new(reason.clone()),
            ))]);
        }
        let tool_call = v1::ToolCallUpdate::new(
            v1::ToolCallId::new(format!("ask-{}", request.invokable)),
            fields,
        );
        let acp_request =
            v1::RequestPermissionRequest::new(self.session_id.clone(), tool_call, options);
        let cx = self.cx.clone();
        async move {
            match cx.send_request(acp_request).block_task().await {
                Ok(response) => match response.outcome {
                    v1::RequestPermissionOutcome::Selected(selected) => {
                        let id = selected.option_id.0.to_string();
                        choices
                            .iter()
                            .find(|(_, choice)| permission_option_id(*choice) == id)
                            .map(|(_, choice)| *choice)
                            .unwrap_or(crate::permissions::PermissionChoice::Cancelled)
                    }
                    _ => crate::permissions::PermissionChoice::Cancelled,
                },
                Err(_) => crate::permissions::PermissionChoice::Cancelled,
            }
        }
    }
}

/// The session's selected actor: created in session metadata, falling
/// back to `[defaults].actor`, then the built-in "default".
pub(crate) fn session_actor(session: &crate::store::Session, fallback: Option<&str>) -> String {
    serde_json::from_str::<serde_json::Value>(&session.metadata)
        .ok()
        .and_then(|meta| meta.get("actor")?.as_str().map(str::to_owned))
        .or_else(|| fallback.map(str::to_owned))
        .unwrap_or_else(|| "default".into())
}

/// The model's context window from the bundled models.dev snapshot;
/// None when the model is unknown (no usage_update is sent then — the
/// RFD's guidance for unknowable window sizes).
pub(crate) fn context_window(model_ref: &str) -> Option<u64> {
    let bare = model_ref.split_once('/')?.1;
    let snapshot = agentkit_models::bundled_snapshot_parsed();
    snapshot.models.get(bare)?.context_window.map(u64::from)
}

/// The text of a stored message: its text blocks joined.
pub fn prompt_text_of(content: &str) -> String {
    serde_json::from_str::<Vec<serde_json::Value>>(content)
        .map(|blocks| crate::agent::turn::prompt_text(&blocks))
        .unwrap_or_default()
}

/// RFC 3339 rendering of a unix-seconds timestamp (ACP `updatedAt`).
fn timestamp(unix_seconds: i64) -> String {
    jiff::Timestamp::from_second(unix_seconds)
        .map(|ts| ts.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client_meta(pairs: &[&str]) -> Option<agent_client_protocol::schema::v1::Meta> {
        let meta: agent_client_protocol::schema::v1::Meta = pairs
            .iter()
            .map(|key| ((*key).to_owned(), serde_json::json!({})))
            .collect();
        Some(meta)
    }

    #[test]
    fn client_advertised_features_are_captured() {
        let meta = client_meta(&["unstable_session_fork", "unstable_session_notices"]);
        let supported = UnstableFeature::advertised_in(&meta);
        assert!(supported.contains(&UnstableFeature::Fork));
        assert!(supported.contains(&UnstableFeature::SessionNotices));
        assert!(!supported.contains(&UnstableFeature::SessionCompaction));
    }

    #[test]
    fn absent_meta_advertises_nothing() {
        assert!(UnstableFeature::advertised_in(&None).is_empty());
        assert!(UnstableFeature::advertised_in(&client_meta(&[])).is_empty());
    }

    #[test]
    fn editor_toggles_are_per_connection() {
        let on = client_tools::EditorToggles {
            fs: true,
            terminal: true,
        };
        let a = ConnectionNegotiation::new(on, McpPool::new());
        let b = ConnectionNegotiation::new(on, McpPool::new());

        assert_eq!(
            editor_command(&a, "/editor terminal false").as_deref(),
            Some("Editor terminal tools: disabled")
        );
        assert!(!a.editor_toggles().terminal, "a toggled off");
        assert!(b.editor_toggles().terminal, "b is untouched");
    }

    #[tokio::test]
    async fn mcp_pools_are_per_connection() {
        let on = client_tools::EditorToggles {
            fs: true,
            terminal: true,
        };
        let a = ConnectionNegotiation::new(on, McpPool::new());
        let b = ConnectionNegotiation::new(on, McpPool::new());

        a.mcp_pool().lock().await.register(
            "only-a",
            crate::config::McpServerConfig::Http {
                url: "http://example.invalid".to_owned(),
            },
        );

        assert_eq!(a.mcp_pool().lock().await.server_names(), vec!["only-a"]);
        assert!(
            b.mcp_pool().lock().await.server_names().is_empty(),
            "a connection's servers never appear on another"
        );
    }
}
