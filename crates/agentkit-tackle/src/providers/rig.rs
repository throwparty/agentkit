//! Shared rig-core plumbing: conversions between tackle model types and rig types.

use super::{ChatMessage, ChatRole};
use rig_core::message::{AssistantContent, Message};

pub(super) fn to_rig_message(message: &ChatMessage) -> Message {
    match message.role {
        ChatRole::User => Message::user(message.text.clone()),
        ChatRole::Assistant => {
            if message.tool_calls.is_empty() {
                Message::assistant(message.text.clone())
            } else {
                let mut content: Vec<AssistantContent> = Vec::new();
                if !message.text.is_empty() {
                    content.push(AssistantContent::text(message.text.clone()));
                }
                for call in &message.tool_calls {
                    let arguments =
                        serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
                    content.push(AssistantContent::tool_call(
                        call.id.clone(),
                        call.name.clone(),
                        arguments,
                    ));
                }
                Message::Assistant { id: None, content }
            }
        }
        ChatRole::Tool => Message::tool_result(
            message.tool_call_id.clone().unwrap_or_default(),
            message.tool_name.clone().unwrap_or_default(),
            message.text.clone(),
        ),
    }
}

/// Extracts the tool calls from a completed choice.
pub(super) fn tool_calls_of(choice: &[AssistantContent]) -> Vec<super::ModelToolCall> {
    choice
        .iter()
        .filter_map(|content| match content {
            AssistantContent::ToolCall(call) => Some(super::ModelToolCall {
                id: call.id.as_str().to_owned(),
                name: call.function.name.clone(),
                arguments: call.function.arguments.to_string(),
            }),
            _ => None,
        })
        .collect()
}
