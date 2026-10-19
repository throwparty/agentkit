//! The `TurnSink` implementation over a live connection: each domain event
//! becomes the corresponding `session/update` notification.

use crate::agent::sink::{ToolCall, ToolCallStatus, ToolCallUpdate, TurnSink};
use crate::invokables::titles::ToolKind;
use crate::state::wire_session_id;
use crate::store::SessionId;
use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, MessageId, SessionInfoUpdate,
    SessionNotification, SessionUpdate, TextContent, ToolCall as WireToolCall, ToolCallContent,
    ToolCallId, ToolCallStatus as WireStatus, ToolCallUpdate as WireToolCallUpdate,
    ToolCallUpdateFields, UsageUpdate,
};
use agent_client_protocol::{Client, ConnectionTo};

/// Reports the turn loop's events over `cx`.
pub struct ConnectionTurnSink {
    cx: ConnectionTo<Client>,
}

impl ConnectionTurnSink {
    pub fn new(cx: ConnectionTo<Client>) -> Self {
        Self { cx }
    }

    fn send(&self, session: &SessionId, update: SessionUpdate) {
        let _ = self
            .cx
            .send_notification(SessionNotification::new(wire_session_id(session), update));
    }
}

fn text_chunk(message_id: &str, text: &str) -> ContentChunk {
    ContentChunk::new(ContentBlock::Text(TextContent::new(text.to_owned())))
        .message_id(MessageId::new(message_id.to_owned()))
}

pub(crate) fn wire_kind(kind: ToolKind) -> agent_client_protocol::schema::v1::ToolKind {
    use agent_client_protocol::schema::v1::ToolKind as Wire;
    match kind {
        ToolKind::Read => Wire::Read,
        ToolKind::Edit => Wire::Edit,
        ToolKind::Execute => Wire::Execute,
        ToolKind::Other => Wire::Other,
    }
}

fn wire_status(status: ToolCallStatus) -> WireStatus {
    match status {
        ToolCallStatus::Pending => WireStatus::Pending,
        ToolCallStatus::InProgress => WireStatus::InProgress,
        ToolCallStatus::Completed => WireStatus::Completed,
        ToolCallStatus::Failed => WireStatus::Failed,
    }
}

impl TurnSink for ConnectionTurnSink {
    fn usage(&self, session: &SessionId, input_tokens: u64, context_size: u64) {
        self.send(
            session,
            SessionUpdate::UsageUpdate(UsageUpdate::new(input_tokens, context_size)),
        );
    }

    fn text_delta(&self, session: &SessionId, message_id: &str, text: &str) {
        self.send(
            session,
            SessionUpdate::AgentMessageChunk(text_chunk(message_id, text)),
        );
    }

    fn thought_delta(&self, session: &SessionId, message_id: &str, text: &str) {
        self.send(
            session,
            SessionUpdate::AgentThoughtChunk(text_chunk(message_id, text)),
        );
    }

    fn retry(&self, session: &SessionId, attempt: u32, message: &str) {
        let card = WireToolCall::new(
            ToolCallId::new(format!("retry-{attempt}-{}", uuid::Uuid::new_v4())),
            format!("Retrying model request (attempt {attempt}): {message}"),
        )
        .status(WireStatus::InProgress);
        self.send(session, SessionUpdate::ToolCall(card));
    }

    fn tool_call(&self, session: &SessionId, call: ToolCall) {
        let wire = WireToolCall::new(ToolCallId::new(call.id), call.title)
            .name(call.name)
            .kind(wire_kind(call.kind))
            .status(wire_status(call.status))
            .raw_input(Some(call.arguments));
        self.send(session, SessionUpdate::ToolCall(wire));
    }

    fn tool_call_update(&self, session: &SessionId, update: ToolCallUpdate) {
        let fields = ToolCallUpdateFields::new()
            .status(wire_status(update.status))
            .content(vec![ToolCallContent::from(ContentBlock::Text(
                TextContent::new(update.content),
            ))]);
        let wire = WireToolCallUpdate::new(ToolCallId::new(update.id), fields);
        self.send(session, SessionUpdate::ToolCallUpdate(wire));
    }

    fn session_info(&self, session: &SessionId, updated_at: &str) {
        self.send(
            session,
            SessionUpdate::SessionInfoUpdate(
                SessionInfoUpdate::new().updated_at(updated_at.to_owned()),
            ),
        );
    }
}
