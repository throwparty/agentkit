//! Session replay: rendering the shared neutral replay stream as the
//! `session/load` notification sequence.

use super::wire_kind;
use crate::acp::common::replay::{walk, ReplayEvent};
use agent_client_protocol::schema::v1::{SessionId, SessionNotification, SessionUpdate};

/// Maps stored messages to replay notifications via the shared neutral walk:
/// user/assistant messages become message chunks sharing the stored message id
/// (stable identity per logical message); tool calls replay as in-progress
/// announcements with their stored result applied as a completed/failed update;
/// system messages replay as agent chunks; compaction summaries are skipped in
/// stable v1 replay.
pub(super) fn replay_updates(
    session_id: &SessionId,
    assembled: &[crate::store::AssembledMessage],
) -> Vec<SessionNotification> {
    walk(assembled)
        .into_iter()
        .flat_map(|event| render(session_id, event))
        .collect()
}

fn render(session_id: &SessionId, event: ReplayEvent) -> Vec<SessionNotification> {
    use agent_client_protocol::schema::v1::{
        ContentBlock, ContentChunk, MessageId, ToolCall, ToolCallContent, ToolCallStatus,
        ToolCallUpdate, ToolCallUpdateFields,
    };

    let mut updates = Vec::new();
    match event {
        ReplayEvent::ToolCall {
            id,
            name,
            arguments,
        } => {
            updates.push(notification(
                session_id,
                SessionUpdate::ToolCall(
                    ToolCall::new(
                        id,
                        crate::invokables::titles::tool_call_title(&name, &arguments),
                    )
                    .name(name.clone())
                    .kind(wire_kind(crate::invokables::titles::tool_kind(&name)))
                    .status(ToolCallStatus::InProgress)
                    .raw_input(Some(arguments)),
                ),
            ));
        }
        ReplayEvent::ToolResult {
            call_id,
            is_error,
            content_json,
        } => {
            let content: Vec<ToolCallContent> =
                serde_json::from_str::<Vec<ContentBlock>>(&content_json)
                    .unwrap_or_default()
                    .into_iter()
                    .map(ToolCallContent::from)
                    .collect();
            let status = if is_error {
                ToolCallStatus::Failed
            } else {
                ToolCallStatus::Completed
            };
            let fields = ToolCallUpdateFields::new().status(status).content(content);
            updates.push(notification(
                session_id,
                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(call_id, fields)),
            ));
        }
        ReplayEvent::Message {
            id,
            is_user,
            blocks_json,
        } => {
            let Ok(blocks) = serde_json::from_str::<Vec<ContentBlock>>(&blocks_json) else {
                return updates; // unparseable stored content: never crash replay
            };
            let message_id = MessageId::new(id);
            for block in blocks {
                let chunk = ContentChunk::new(block).message_id(message_id.clone());
                let update = if is_user {
                    SessionUpdate::UserMessageChunk(chunk)
                } else {
                    SessionUpdate::AgentMessageChunk(chunk)
                };
                updates.push(notification(session_id, update));
            }
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
