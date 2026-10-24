//! Session replay: mapping stored messages to the notification sequence `session/load` sends.

use super::wire_kind;
use agent_client_protocol::schema::v1::{SessionId, SessionNotification, SessionUpdate};

/// Maps stored messages to replay notifications: user/assistant messages
/// become message chunks (all blocks, sharing the stored message id —
/// stable identity per logical message); tool calls replay as in-progress
/// announcements, with their stored result applied as a completed/failed
/// update; seeds replay as agent chunks; compaction summaries are skipped
/// in stable v1 replay.
pub(super) fn replay_updates(
    session_id: &SessionId,
    assembled: &[crate::store::AssembledMessage],
) -> Vec<SessionNotification> {
    use agent_client_protocol::schema::v1::{
        ContentBlock, ContentChunk, MessageId, ToolCall, ToolCallContent, ToolCallStatus,
        ToolCallUpdate, ToolCallUpdateFields,
    };
    use std::collections::HashSet;

    let mut updates = Vec::new();
    let mut pending_calls: HashSet<String> = HashSet::new();
    for assembled in assembled {
        let message = &assembled.message;
        let message_id = MessageId::new(message.id.clone());

        // Tool messages carry raw JSON (arguments/results), not content
        // blocks, so handle them before the block parser.
        match message.role {
            crate::store::Role::ToolCall => {
                // Announce the call as it stood live: in progress until a
                // matching result is replayed.
                let name = message.tool_name.clone().unwrap_or_default();
                let arguments: serde_json::Value =
                    serde_json::from_str(&message.content).unwrap_or(serde_json::Value::Null);
                updates.push(notification(
                    session_id,
                    SessionUpdate::ToolCall(
                        ToolCall::new(
                            message.id.clone(),
                            crate::invokables::titles::tool_call_title(&name, &arguments),
                        )
                        .name(name.clone())
                        .kind(wire_kind(crate::invokables::titles::tool_kind(&name)))
                        .status(ToolCallStatus::InProgress)
                        .raw_input(Some(arguments)),
                    ),
                ));
                pending_calls.insert(message.id.clone());
                continue;
            }
            crate::store::Role::ToolResult => {
                // Pair on the stored call id; an orphan result has no call
                // to update and is skipped.
                let Some(call_id) = message.tool_call_id.clone() else {
                    continue;
                };
                if !pending_calls.remove(&call_id) {
                    continue;
                }
                let content: Vec<ToolCallContent> =
                    match serde_json::from_str::<Vec<ContentBlock>>(&message.content) {
                        Ok(blocks) => blocks.into_iter().map(ToolCallContent::from).collect(),
                        Err(_) => Vec::new(),
                    };
                let status = if message.is_error == Some(true) {
                    ToolCallStatus::Failed
                } else {
                    ToolCallStatus::Completed
                };
                let fields = ToolCallUpdateFields::new().status(status).content(content);
                updates.push(notification(
                    session_id,
                    SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(call_id, fields)),
                ));
                continue;
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{AssembledMessage, Message, Role, TurnKind};
    use agent_client_protocol::schema::v1::{
        ContentBlock, SessionUpdate, ToolCallContent, ToolCallStatus,
    };

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
    fn a_paired_result_completes_the_call_with_its_content() {
        let assembled = vec![call("call-1"), result("call-1", false)];
        let updates = replay_updates(&SessionId::new("session"), &assembled);

        let SessionUpdate::ToolCall(announced) = &updates[0].update else {
            panic!("expected a tool call announcement");
        };
        assert_eq!(announced.tool_call_id.0.as_ref(), "call-1");
        assert_eq!(announced.status, ToolCallStatus::InProgress);
        assert_eq!(
            announced.raw_input,
            Some(serde_json::json!({ "path": "a.txt" }))
        );

        let SessionUpdate::ToolCallUpdate(completed) = &updates[1].update else {
            panic!("expected a tool call update");
        };
        assert_eq!(completed.tool_call_id.0.as_ref(), "call-1");
        assert_eq!(completed.fields.status, Some(ToolCallStatus::Completed));
        let content = completed.fields.content.as_ref().unwrap();
        let ToolCallContent::Content(content) = &content[0] else {
            panic!("expected text content");
        };
        let ContentBlock::Text(text) = &content.content else {
            panic!("expected a text block");
        };
        assert_eq!(text.text, "tool output");
    }

    #[test]
    fn an_error_result_fails_the_call() {
        let assembled = vec![call("call-1"), result("call-1", true)];
        let updates = replay_updates(&SessionId::new("session"), &assembled);

        let SessionUpdate::ToolCallUpdate(failed) = &updates[1].update else {
            panic!("expected a tool call update");
        };
        assert_eq!(failed.fields.status, Some(ToolCallStatus::Failed));
    }

    #[test]
    fn a_call_without_a_result_stays_in_progress() {
        let assembled = vec![call("call-1")];
        let updates = replay_updates(&SessionId::new("session"), &assembled);

        assert_eq!(updates.len(), 1);
        let SessionUpdate::ToolCall(announced) = &updates[0].update else {
            panic!("expected a tool call announcement");
        };
        assert_eq!(announced.status, ToolCallStatus::InProgress);
    }

    #[test]
    fn an_orphan_result_is_ignored() {
        let assembled = vec![result("missing-call", false)];
        assert!(replay_updates(&SessionId::new("session"), &assembled).is_empty());
    }
}
