//! Turn-DAG replay walk producing version-neutral events.

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
        content_json: String,
    },
    /// A user, assistant, or system message. `is_user` selects the wire chunk
    /// kind; the rest is the stored content-block JSON.
    Message {
        id: String,
        is_user: bool,
        blocks_json: String,
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
                    content_json: message.content.clone(),
                });
            }
            Role::User | Role::Assistant | Role::System => {
                events.push(ReplayEvent::Message {
                    id: message.id.clone(),
                    is_user: matches!(message.role, Role::User),
                    blocks_json: message.content.clone(),
                });
            }
        }
    }
    events
}
