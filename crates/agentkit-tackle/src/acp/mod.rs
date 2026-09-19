//! ACP server surface: protocol handling via the SDK's callback builders.
//!
//! Handlers are registered per request type and tried in order; shared
//! state rides in `Arc<TackleState>` captured by each callback, and
//! per-connection negotiation state in `Arc<ConnectionNegotiation>`. The
//! stdio transport is the normal launch mode (one process per client
//! connection); HTTP arrives with T-036.

use crate::config::Loaded;
use crate::loader::Definitions;
use crate::store::SessionStore;
#[cfg(feature = "unstable")]
use agent_client_protocol::schema::v1::SessionForkCapabilities;
use agent_client_protocol::schema::v1::{
    AgentCapabilities, CloseSessionResponse, DeleteSessionResponse, InitializeRequest,
    InitializeResponse, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse,
    McpCapabilities, NewSessionRequest, NewSessionResponse, PromptCapabilities,
    ResumeSessionRequest, ResumeSessionResponse, SessionCapabilities, SessionCloseCapabilities,
    SessionDeleteCapabilities, SessionId, SessionInfo, SessionListCapabilities,
    SessionNotification, SessionResumeCapabilities, SessionUpdate,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{Agent, Error, Stdio};
use std::collections::BTreeSet;
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

/// Per-connection negotiation state, captured at initialize and consulted
/// by every update sender.
#[derive(Default)]
pub struct ConnectionNegotiation {
    supported: Mutex<BTreeSet<UnstableFeature>>,
}

impl ConnectionNegotiation {
    fn capture(&self, client_meta: &Option<agent_client_protocol::schema::v1::Meta>) {
        *self.supported.lock().expect("negotiation poisoned") =
            UnstableFeature::advertised_in(client_meta);
    }

    /// Whether the client advertised support for `feature`.
    pub fn supports(&self, feature: UnstableFeature) -> bool {
        self.supported
            .lock()
            .expect("negotiation poisoned")
            .contains(&feature)
    }
}

/// State shared across one connection's handlers.
pub struct TackleState {
    pub db: SessionStore,
    pub config: Loaded,
    pub definitions: Definitions,
}

impl TackleState {
    /// The capability map: the stable v1 surface plus the unstable
    /// features tackle implements (advertised unconditionally — the gate
    /// applies to *sending* RFD-shaped updates, per FR-002).
    fn capabilities(&self) -> AgentCapabilities {
        let mut unstable: agent_client_protocol::schema::v1::Meta =
            agent_client_protocol::schema::v1::Meta::new();
        #[cfg(feature = "unstable")]
        for feature in [
            UnstableFeature::Fork,
            UnstableFeature::SessionCompaction,
            UnstableFeature::SessionNotices,
        ] {
            unstable.insert(feature.meta_key().to_owned(), serde_json::json!({}));
        }
        AgentCapabilities::new()
            .load_session(true)
            .prompt_capabilities(PromptCapabilities::new().image(true).audio(false))
            .mcp_capabilities(McpCapabilities::new().http(true).sse(false))
            .session_capabilities({
                let capabilities = SessionCapabilities::new()
                    .list(Some(SessionListCapabilities::default()))
                    .close(Some(SessionCloseCapabilities::default()))
                    .resume(Some(SessionResumeCapabilities::default()))
                    .delete(Some(SessionDeleteCapabilities::default()));
                #[cfg(feature = "unstable")]
                let capabilities = capabilities.fork(Some(SessionForkCapabilities::default()));
                capabilities
            })
            .meta(unstable)
    }
}

/// Prefixes a session id for the wire (the storage ADR's serialization
/// boundary).
fn wire_session_id(id: &crate::store::SessionId) -> SessionId {
    SessionId::new(format!("sess_{id}"))
}

/// Strips the wire prefix back to the bare store id.
fn store_session_id(id: &SessionId) -> String {
    id.to_string()
        .strip_prefix("sess_")
        .unwrap_or(id.to_string().as_str())
        .to_owned()
}

/// Runs the ACP agent over stdio until the transport closes.
pub async fn run_stdio(state: Arc<TackleState>) -> agent_client_protocol::Result<()> {
    let negotiation = Arc::new(ConnectionNegotiation::default());
    let negotiation_for_init = negotiation.clone();
    let state_for_init = state.clone();

    // The connection's lease-owner id — consumed from T-015 onward, when
    // turns acquire leases on the session.
    let _owner = Arc::new(format!("stdio-{}", uuid::Uuid::new_v4()));
    let state_for_new = state.clone();
    let state_for_list = state.clone();
    let state_for_close = state.clone();
    let state_for_delete = state.clone();
    let state_for_load = state.clone();

    Agent
        .builder()
        .name("tackle")
        .on_receive_request(
            async move |request: InitializeRequest, responder, _cx| {
                // Protocol version negotiation: tackle implements v1, so it
                // responds with v1 regardless of the client's latest; a
                // v1-only client proceeds, a v2-only client disconnects.
                negotiation_for_init.capture(&request.client_capabilities.meta);
                responder.respond(
                    InitializeResponse::new(ProtocolVersion::V1)
                        .agent_capabilities(state_for_init.capabilities()),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: NewSessionRequest, responder, _cx| {
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
                // creation never blocks on them.
                responder.respond(NewSessionResponse::new(wire_session_id(&session.id)))
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
                        _cx| {
                state_for_close
                    .db
                    .close_session(&store_session_id(&request.session_id))
                    .await
                    .map_err(|err| Error::internal_error().data(err.to_string()))?;
                responder.respond(CloseSessionResponse::new())
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
                responder.respond(DeleteSessionResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: LoadSessionRequest, responder, cx| {
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
                responder.respond(LoadSessionResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: ResumeSessionRequest, responder, _cx| {
                // Resume reattaches WITHOUT replay (FR-003); config state
                // in the response arrives with T-031.
                responder.respond(ResumeSessionResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_to(Stdio::new())
        .await
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
        let blocks: Vec<ContentBlock> = match serde_json::from_str(&message.content) {
            Ok(blocks) => blocks,
            Err(_) => continue, // unparseable stored content: skip, never crash replay
        };
        let message_id = MessageId::new(message.id.clone());

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
            }
            crate::store::Role::ToolResult => {
                // Collapsed into the tool_call update above.
            }
        }
    }
    updates
}

fn notification(session_id: &SessionId, update: SessionUpdate) -> SessionNotification {
    SessionNotification::new(session_id.clone(), update)
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
}
