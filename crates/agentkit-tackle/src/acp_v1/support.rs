//! Shared handler support: the connection-context alias, the turn-runtime
//! builder, and the assistant-message and notice emitters the request
//! handlers share.

use super::capabilities::UnstableFeature;
use super::client_tools;
use super::negotiation::ConnectionNegotiation;
use super::permission::AcpPermissionRequester;
use super::wire_notice;
use super::ConnectionTurnSink;
use crate::agent::config_options::Notice;
use crate::agent::runtime::TurnRuntime;
use crate::content::timestamp;
use crate::state::TackleState;
use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, MessageId, SessionId, SessionInfoUpdate, SessionNotification,
    SessionUpdate, TextContent,
};
use agent_client_protocol::{ConnectionTo, Error};
use std::sync::Arc;

/// The ACP connection context for an agent-role connection.
pub(super) type Cx = ConnectionTo<agent_client_protocol::Client>;

/// Builds the turn runtime for a prompt or direct invocation from the live
/// connection: the pool, sink, permission requester, client-tool executor
/// and catalogue, and the detached-firing notice sinks.
pub(super) fn turn_runtime(
    negotiation: &Arc<ConnectionNegotiation>,
    cx: &Cx,
    session_id: SessionId,
) -> TurnRuntime {
    let cx = cx.clone();
    TurnRuntime {
        pool: negotiation.mcp_pool(),
        sink: Arc::new(ConnectionTurnSink::new(cx.clone())),
        requester: Arc::new(AcpPermissionRequester {
            cx: cx.clone(),
            session_id,
        }),
        executor: Arc::new(client_tools::ConnectionClientTools { cx: cx.clone() }),
        tracker: negotiation.clone(),
        compaction_notices: super::notices::connection_notices(&cx, negotiation, "compaction"),
        titling_notices: super::notices::connection_notices(&cx, negotiation, "titling"),
        client_tools: client_tools::catalog(
            negotiation.client_caps(),
            negotiation.editor_toggles(),
        ),
    }
}

/// Persists a harness-authored assistant text message on a turn. Returns
/// the stored message so the caller can reuse its id for the streaming
/// chunk.
pub(super) async fn append_assistant_text(
    state: &TackleState,
    turn_id: &crate::store::TurnId,
    text: &str,
) -> Result<crate::store::Message, Error> {
    state
        .db
        .append_message(
            turn_id,
            crate::store::Role::Assistant,
            &serde_json::json!([{ "type": "text", "text": text }]).to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))
}

/// Streams one `agent_message_chunk` carrying `text` under `message_id`.
pub(super) fn send_agent_message_chunk(
    cx: &Cx,
    wire_session: &SessionId,
    message_id: &str,
    text: &str,
) {
    let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new(text.to_owned())))
        .message_id(MessageId::new(message_id.to_owned()));
    let _ = cx.send_notification(SessionNotification::new(
        wire_session.clone(),
        SessionUpdate::AgentMessageChunk(chunk),
    ));
}

/// Emits the session's `updatedAt` after a turn, harness-authored turns
/// included.
pub(super) async fn send_updated_at(
    state: &TackleState,
    cx: &Cx,
    wire_session: &SessionId,
    store_session: &crate::store::SessionId,
) {
    let updated = SessionInfoUpdate::new().updated_at(
        state
            .db
            .get_session(store_session)
            .await
            .ok()
            .flatten()
            .map(|session| timestamp(session.updated_at))
            .unwrap_or_default(),
    );
    let _ = cx.send_notification(SessionNotification::new(
        wire_session.clone(),
        SessionUpdate::SessionInfoUpdate(updated),
    ));
}

/// Emits a batch of discovery notices, gated once on the client's advertised
/// notices capability.
pub(super) fn send_notices(
    cx: &Cx,
    negotiation: &ConnectionNegotiation,
    wire_session: &SessionId,
    notices: &[Notice],
) {
    if !negotiation.supports(UnstableFeature::SessionNotices) {
        return;
    }
    for notice in notices {
        let _ = cx.send_notification(SessionNotification::new(
            wire_session.clone(),
            SessionUpdate::Notice(wire_notice(notice)),
        ));
    }
}
