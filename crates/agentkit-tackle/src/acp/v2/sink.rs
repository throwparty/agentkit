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

/// A usage update with the cumulative cost attached only when known.
fn usage_update(used: u64, size: u64, cost: Option<f64>) -> UsageUpdate {
    let update = UsageUpdate::new(used, size);
    match cost {
        Some(amount) => update.cost(Cost::new(amount, "USD")),
        None => update,
    }
}

impl TurnSink for ConnectionTurnSink {
    fn usage(&self, session: &SessionId, input_tokens: u64, context_size: u64, cost: Option<f64>) {
        self.send(
            session,
            SessionUpdate::UsageUpdate(usage_update(input_tokens, context_size, cost)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_is_attached_only_when_known() {
        let priced = usage_update(10, 100, Some(0.42));
        let cost = priced.cost.expect("cost present");
        assert_eq!(cost.amount, 0.42);
        assert_eq!(cost.currency, "USD");

        assert!(usage_update(10, 100, None).cost.is_none());
    }

    #[test]
    fn chunks_carry_the_message_id() {
        let chunk = text_chunk("message-1", "hello");
        assert_eq!(chunk.message_id, MessageId::new("message-1"));
        let ContentBlock::Text(text) = chunk.content else {
            panic!("expected a text block");
        };
        assert_eq!(text.text, "hello");
    }
}
