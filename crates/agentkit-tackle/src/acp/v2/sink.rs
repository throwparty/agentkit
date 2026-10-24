//! The `TurnSink` implementation over a live v2 connection: each domain event
//! becomes the corresponding v2 `session/update` notification.

use crate::agent::sink::{ToolCall, ToolCallUpdate, TurnSink};
use crate::store::SessionId;
use agent_client_protocol::role::acp::Client;
use agent_client_protocol::schema::v2::{
    ContentBlock, ContentChunk, Cost, MessageId, Notice, NoticeSeverity, SessionInfoUpdate,
    SessionUpdate, TextContent, UpdateSessionNotification, UsageUpdate,
};
use agent_client_protocol::V2ConnectionTo;

/// Reports the turn loop's events over `cx`.
pub struct ConnectionTurnSink {
    cx: V2ConnectionTo<Client>,
}

impl ConnectionTurnSink {
    pub fn new(cx: V2ConnectionTo<Client>) -> Self {
        Self { cx }
    }

    fn send(&self, session: &SessionId, update: SessionUpdate) {
        let _ = self.cx.send_notification(UpdateSessionNotification::new(
            super::wire_session_id(session),
            update,
        ));
    }
}

fn text_chunk(message_id: &str, text: &str) -> ContentChunk {
    ContentChunk::new(
        ContentBlock::Text(TextContent::new(text.to_owned())),
        MessageId::new(message_id.to_owned()),
    )
}

impl TurnSink for ConnectionTurnSink {
    fn usage(&self, session: &SessionId, input_tokens: u64, context_size: u64, cost: Option<f64>) {
        let update = UsageUpdate::new(input_tokens, context_size);
        let update = match cost {
            Some(amount) => update.cost(Cost::new(amount, "USD")),
            None => update,
        };
        self.send(session, SessionUpdate::UsageUpdate(update));
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
        self.send(
            session,
            SessionUpdate::Notice(Notice::new(
                NoticeSeverity::Info,
                format!("Retrying model request (attempt {attempt}): {message}"),
            )),
        );
    }

    fn tool_call(&self, _session: &SessionId, _call: ToolCall) {}

    fn tool_call_update(&self, _session: &SessionId, _update: ToolCallUpdate) {}

    fn session_info(&self, session: &SessionId, updated_at: &str) {
        self.send(
            session,
            SessionUpdate::SessionInfoUpdate(
                SessionInfoUpdate::new().updated_at(updated_at.to_owned()),
            ),
        );
    }
}
