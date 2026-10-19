//! Session replay: mapping stored messages to the notification sequence `session/load` sends.

use super::wire_kind;
use agent_client_protocol::schema::v1::{SessionId, SessionNotification, SessionUpdate};

/// Maps stored messages to replay notifications: user/assistant messages
/// become message chunks (all blocks, sharing the stored message id —
/// stable identity per logical message); tool call/result pairs collapse
/// into one completed tool-call update; seeds replay as agent chunks;
/// compaction summaries are skipped in stable v1 replay.
pub(super) fn replay_updates(
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
                // The pair collapses: one completed tool-call update. The
                // title is derived from the stored arguments, matching the
                // live announcement.
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

