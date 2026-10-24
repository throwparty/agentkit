//! The `TurnSink` implementation over a live v2 connection: each domain event
//! becomes the corresponding v2 `session/update` notification.

use crate::agent::sink::{ToolCall, ToolCallUpdate, TurnSink};
use crate::store::SessionId;
use agent_client_protocol::role::acp::Client;
use agent_client_protocol::schema::v2::{
    Content, ContentBlock, ContentChunk, Cost, MessageId, Notice, NoticeSeverity,
    SessionInfoUpdate, SessionUpdate, TextContent, ToolCallContent, ToolCallId,
    ToolCallStatus as WireStatus, ToolCallUpdate as WireToolCallUpdate, ToolKind as WireKind,
    UpdateSessionNotification, UsageUpdate,
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

pub(super) fn wire_kind(kind: crate::invokables::titles::ToolKind) -> WireKind {
    use crate::invokables::titles::ToolKind;
    match kind {
        ToolKind::Read => WireKind::Read,
        ToolKind::Edit => WireKind::Edit,
        ToolKind::Execute => WireKind::Execute,
        ToolKind::Other => WireKind::Other,
    }
}

/// The create upsert for a tool call: an update for an unseen id is itself the
/// create in v2.
fn announce(call: ToolCall) -> WireToolCallUpdate {
    WireToolCallUpdate::new(ToolCallId::new(call.id))
        .title(call.title)
        .name(call.name)
        .kind(wire_kind(call.kind))
        .status(wire_status(call.status))
        .raw_input(call.arguments)
}

fn wire_status(status: crate::agent::sink::ToolCallStatus) -> WireStatus {
    use crate::agent::sink::ToolCallStatus;
    match status {
        ToolCallStatus::Pending => WireStatus::Pending,
        ToolCallStatus::InProgress => WireStatus::InProgress,
        ToolCallStatus::Completed => WireStatus::Completed,
        ToolCallStatus::Failed => WireStatus::Failed,
    }
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

    fn tool_call(&self, session: &SessionId, call: ToolCall) {
        self.send(session, SessionUpdate::ToolCallUpdate(announce(call)));
    }

    fn tool_call_update(&self, session: &SessionId, update: ToolCallUpdate) {
        let content = ToolCallContent::Content(Box::new(Content::new(ContentBlock::Text(
            TextContent::new(update.content),
        ))));
        let wire = WireToolCallUpdate::new(ToolCallId::new(update.id))
            .status(wire_status(update.status))
            .content(vec![content]);
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

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::MaybeUndefined;

    #[test]
    fn cost_is_attached_only_when_known() {
        let priced = usage_update(10, 100, Some(0.42));
        let cost = priced.cost.expect("cost present");
        assert_eq!(cost.amount, 0.42);
        assert_eq!(cost.currency, "USD");

        assert!(usage_update(10, 100, None).cost.is_none());
    }

    #[test]
    fn a_tool_call_is_an_upsert_keyed_by_id_with_a_title() {
        let call = ToolCall {
            id: "call-1".to_owned(),
            title: "Read a.txt".to_owned(),
            name: "fs.read".to_owned(),
            kind: crate::invokables::titles::ToolKind::Read,
            status: crate::agent::sink::ToolCallStatus::InProgress,
            arguments: serde_json::json!({ "path": "a.txt" }),
        };
        let wire = announce(call);
        assert_eq!(wire.tool_call_id, ToolCallId::new("call-1"));
        assert!(matches!(&wire.title, MaybeUndefined::Value(t) if t == "Read a.txt"));
        assert!(matches!(wire.status, MaybeUndefined::Value(WireStatus::InProgress)));
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
