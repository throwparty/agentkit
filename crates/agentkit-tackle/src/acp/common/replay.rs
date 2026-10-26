//! Turn-DAG replay walk producing version-neutral events.

use super::content::{self, Block};
use crate::store::{AssembledMessage, Role};
use std::collections::HashSet;

/// One step of a replayed session, before it is rendered to a wire update.
pub(crate) enum ReplayEvent {
    /// A tool call announcement: its stored id, tool name, and arguments.
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    /// A terminal update for a previously announced call.
    ToolResult {
        call_id: String,
        is_error: bool,
        content: Vec<Block>,
    },
    /// A user, assistant, or system message. `is_user` selects the wire chunk
    /// kind; the rest is the stored content-block JSON.
    Message {
        id: String,
        is_user: bool,
        blocks: Vec<Block>,
    },
}

/// Walks the assembled session in order, emitting one neutral event per stored
/// item. Tool results are paired with a pending call; a result with no pending
/// call is dropped.
pub(crate) fn walk(assembled: &[AssembledMessage]) -> Vec<ReplayEvent> {
    let mut events = Vec::new();
    let mut pending: HashSet<String> = HashSet::new();
    for assembled in assembled {
        let message = &assembled.message;
        match message.role {
            Role::ToolCall => {
                let name = message.tool_name.clone().unwrap_or_default();
                let arguments =
                    serde_json::from_str(&message.content).unwrap_or(serde_json::Value::Null);
                pending.insert(message.id.clone());
                events.push(ReplayEvent::ToolCall {
                    id: message.id.clone(),
                    name,
                    arguments,
                });
            }
            Role::ToolResult => {
                let Some(call_id) = message.tool_call_id.clone() else {
                    continue;
                };
                if !pending.remove(&call_id) {
                    continue;
                }
                events.push(ReplayEvent::ToolResult {
                    call_id,
                    is_error: message.is_error == Some(true),
                    content: content::decode(&message.content),
                });
            }
            Role::User | Role::Assistant | Role::System => {
                events.push(ReplayEvent::Message {
                    id: message.id.clone(),
                    is_user: matches!(message.role, Role::User),
                    blocks: content::decode(&message.content),
                });
            }
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{AssembledMessage, Message, Role, TurnKind};

    fn stored(role: Role, content: &str) -> AssembledMessage {
        AssembledMessage {
            message: Message {
                id: uuid::Uuid::new_v4().to_string(),
                turn_id: "turn-1".to_owned(),
                role,
                content: content.to_owned(),
                tool_name: None,
                tool_call_id: None,
                is_error: None,
                position: 0,
                created_at: 0,
            },
            turn_kind: TurnKind::Interaction,
        }
    }

    #[test]
    fn messages_keep_their_order_and_identity() {
        let mut user = stored(Role::User, r#"[{"type":"text","text":"hi"}]"#);
        user.message.id = "user-1".to_owned();
        let mut assistant = stored(Role::Assistant, r#"[{"type":"text","text":"ok"}]"#);
        assistant.message.id = "assistant-1".to_owned();
        let events = walk(&[user, assistant]);
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], ReplayEvent::Message { id, is_user: true, .. } if id == "user-1"));
        assert!(matches!(&events[1], ReplayEvent::Message { id, is_user: false, .. } if id == "assistant-1"));
    }

    #[test]
    fn a_result_pairs_with_its_call_and_orphans_are_dropped() {
        let mut call = stored(Role::ToolCall, r#"{"path":"a"}"#);
        call.message.id = "call-1".to_owned();
        call.message.tool_name = Some("fs.read".to_owned());
        let mut result = stored(Role::ToolResult, r#"[{"type":"text","text":"out"}]"#);
        result.message.tool_call_id = Some("call-1".to_owned());
        let orphan = {
            let mut orphan = stored(Role::ToolResult, "[]");
            orphan.message.tool_call_id = Some("missing".to_owned());
            orphan
        };
        let events = walk(&[call, result, orphan]);
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], ReplayEvent::ToolCall { id, .. } if id == "call-1"));
        assert!(matches!(&events[1], ReplayEvent::ToolResult { call_id, .. } if call_id == "call-1"));
    }

    #[test]
    fn a_duplicate_result_is_dropped() {
        let mut call = stored(Role::ToolCall, "{}");
        call.message.id = "call-1".to_owned();
        let mut first = stored(Role::ToolResult, "[]");
        first.message.tool_call_id = Some("call-1".to_owned());
        let mut second = stored(Role::ToolResult, "[]");
        second.message.tool_call_id = Some("call-1".to_owned());
        let events = walk(&[call, first, second]);
        assert_eq!(events.len(), 2);
    }
}
