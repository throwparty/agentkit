//! Session replay: rendering the shared neutral replay stream as the v2
//! `session/update` notification sequence.
//!
//! v2 identity is carried per message (`messageId`), so replay uses whole-
//! message upserts rather than chunks: a retained message replays with the
//! same id it was stored under, and no chunk is ever sent during replay.
//! Tool calls replay as keyed upserts: the call creates the card, the result
//! patches its status and content.

use crate::acp::common::replay::{walk, ReplayEvent};
use crate::invokables::titles;
use agent_client_protocol::schema::v2::{
    AgentMessage, Content, ContentBlock, MessageId, SessionId, SessionUpdate, ToolCallContent,
    ToolCallId, ToolCallStatus, ToolCallUpdate, UpdateSessionNotification, UserMessage,
};

/// Maps stored messages to replay notifications via the shared neutral walk.
pub(super) fn replay_updates(
    session_id: &SessionId,
    assembled: &[crate::store::AssembledMessage],
) -> Vec<UpdateSessionNotification> {
    walk(assembled)
        .into_iter()
        .flat_map(|event| render(session_id, event))
        .collect()
}

fn wire_block(block: &crate::acp::common::content::Block) -> ContentBlock {
    match serde_json::from_value(block.to_value()) {
        Ok(block) => block,
        Err(_) => ContentBlock::Text(agent_client_protocol::schema::v2::TextContent::new(
            block.as_text().unwrap_or("[unsupported content]"),
        )),
    }
}

fn render(session_id: &SessionId, event: ReplayEvent) -> Vec<UpdateSessionNotification> {
    let update = match event {
        ReplayEvent::ToolCall {
            id,
            name,
            arguments,
        } => SessionUpdate::ToolCallUpdate(
            ToolCallUpdate::new(ToolCallId::new(id))
                .title(titles::tool_call_title(&name, &arguments))
                .name(name.clone())
                .kind(super::sink::wire_kind(titles::tool_kind(&name)))
                .status(ToolCallStatus::InProgress)
                .raw_input(arguments),
        ),
        ReplayEvent::ToolResult {
            call_id,
            is_error,
            content,
        } => {
            let content: Vec<ToolCallContent> = content
                .iter()
                .map(|block| ToolCallContent::Content(Box::new(Content::new(wire_block(block)))))
                .collect();
            let status = if is_error {
                ToolCallStatus::Failed
            } else {
                ToolCallStatus::Completed
            };
            SessionUpdate::ToolCallUpdate(
                ToolCallUpdate::new(ToolCallId::new(call_id))
                    .status(status)
                    .content(content),
            )
        }
        ReplayEvent::Message {
            id,
            is_user,
            blocks,
        } => {
            let message_id = MessageId::new(id);
            let blocks: Vec<ContentBlock> = blocks.iter().map(wire_block).collect();
            if is_user {
                SessionUpdate::UserMessage(UserMessage::new(message_id).content(blocks))
            } else {
                SessionUpdate::AgentMessage(AgentMessage::new(message_id).content(blocks))
            }
        }
    };
    vec![UpdateSessionNotification::new(
        session_id.clone(),
        update,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{AssembledMessage, Message, Role, TurnKind};

    fn stored(role: Role, content: impl Into<String>) -> AssembledMessage {
        AssembledMessage {
            message: Message {
                id: uuid::Uuid::new_v4().to_string(),
                turn_id: "turn-1".to_owned(),
                role,
                content: content.into(),
                tool_name: None,
                tool_call_id: None,
                is_error: None,
                position: 0,
                created_at: 0,
            },
            turn_kind: TurnKind::Interaction,
        }
    }

    fn call(id: &str) -> AssembledMessage {
        let mut message = stored(Role::ToolCall, r#"{"path":"a.txt"}"#);
        message.message.id = id.to_owned();
        message.message.tool_name = Some("fs.read".to_owned());
        message
    }

    fn result(call_id: &str, is_error: bool) -> AssembledMessage {
        let mut message = stored(
            Role::ToolResult,
            serde_json::json!([{ "type": "text", "text": "tool output" }]).to_string(),
        );
        message.message.tool_call_id = Some(call_id.to_owned());
        message.message.tool_name = Some("fs.read".to_owned());
        message.message.is_error = Some(is_error);
        message
    }

    #[test]
    fn a_call_and_result_replay_as_a_create_then_patch() {
        let updates = replay_updates(
            &SessionId::new("session"),
            &[call("call-1"), result("call-1", false)],
        );
        assert_eq!(updates.len(), 2);

        let SessionUpdate::ToolCallUpdate(announced) = &updates[0].update else {
            panic!("expected a tool call update");
        };
        assert_eq!(announced.tool_call_id, ToolCallId::new("call-1"));
        assert!(matches!(
            announced.status,
            agent_client_protocol::schema::MaybeUndefined::Value(ToolCallStatus::InProgress)
        ));

        let SessionUpdate::ToolCallUpdate(completed) = &updates[1].update else {
            panic!("expected a tool call update");
        };
        assert_eq!(completed.tool_call_id, ToolCallId::new("call-1"));
        assert!(matches!(
            completed.status,
            agent_client_protocol::schema::MaybeUndefined::Value(ToolCallStatus::Completed)
        ));
        let agent_client_protocol::schema::MaybeUndefined::Value(content) = &completed.content
        else {
            panic!("expected content");
        };
        let ToolCallContent::Content(content) = &content[0] else {
            panic!("expected content");
        };
        let ContentBlock::Text(text) = &content.content else {
            panic!("expected a text block");
        };
        assert_eq!(text.text, "tool output");
    }

    #[test]
    fn an_error_result_fails_the_call() {
        let updates = replay_updates(
            &SessionId::new("session"),
            &[call("call-1"), result("call-1", true)],
        );
        let SessionUpdate::ToolCallUpdate(failed) = &updates[1].update else {
            panic!("expected a tool call update");
        };
        assert!(matches!(
            failed.status,
            agent_client_protocol::schema::MaybeUndefined::Value(ToolCallStatus::Failed)
        ));
    }

    #[test]
    fn a_message_replays_as_a_whole_message_upsert() {
        let updates = replay_updates(
            &SessionId::new("session"),
            &[stored(
                Role::User,
                serde_json::json!([{ "type": "text", "text": "hi" }]).to_string(),
            )],
        );
        let SessionUpdate::UserMessage(message) = &updates[0].update else {
            panic!("expected a user message");
        };
        assert!(!message.message_id.0.is_empty());
    }
}
