//! Shared rig-core plumbing: conversions between tackle model types and rig types.

use super::{ChatContent, ChatMessage, ChatRole, ModelError};
use crate::invokables::tool_name::{InvokableName, ToolNameMap, WireToolName};
use rig_core::message::{
    AssistantContent, AssistantMessage, CallId, ImageMediaType, Message, MimeType, ToolName,
    UserContent,
};

pub(super) fn to_rig_message(
    message: &ChatMessage,
    names: &mut ToolNameMap,
) -> Result<Message, ModelError> {
    match message.role {
        ChatRole::User => Ok(Message::User {
            content: message
                .content
                .iter()
                .map(|content| match content {
                    ChatContent::Text(text) => UserContent::text(text.clone()),
                    ChatContent::Image { url } => UserContent::image_url(url.clone(), None, None),
                })
                .collect(),
        }),
        ChatRole::Assistant => {
            let mut content: Vec<AssistantContent> = message
                .content
                .iter()
                .filter_map(|content| match content {
                    ChatContent::Text(text) => Some(AssistantContent::text(text.clone())),
                    ChatContent::Image { url } => assistant_image(url),
                })
                .collect();
            for call in &message.tool_calls {
                let arguments =
                    serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
                let wire = names
                    .encode(&InvokableName::new(&call.name))
                    .map_err(|error| ModelError::Context(format!("tool name: {error}")))?;
                content.push(AssistantContent::tool_call(
                    call.id.clone(),
                    tool_name(wire.as_str())?,
                    arguments,
                ));
            }
            Ok(Message::Assistant(AssistantMessage::new(content)))
        }
        ChatRole::Tool => {
            let wire = names
                .encode(&InvokableName::new(
                    message.tool_name.as_deref().unwrap_or_default(),
                ))
                .map_err(|error| ModelError::Context(format!("tool name: {error}")))?;
            Ok(Message::tool_result(
                CallId::from_wire(message.tool_call_id.clone().unwrap_or_default()),
                tool_name(wire.as_str())?,
                message.text(),
            ))
        }
    }
}

fn tool_name(name: &str) -> Result<ToolName, ModelError> {
    ToolName::new(name).map_err(|_| ModelError::Context(format!("empty tool name: {name:?}")))
}

/// Assistant image history only carries data URLs; rig's assistant image
/// constructor accepts base64 plus a known media type, not arbitrary URLs.
fn assistant_image(url: &str) -> Option<AssistantContent> {
    let (mime, data) = url.strip_prefix("data:")?.split_once(";base64,")?;
    let media_type = ImageMediaType::from_mime_type(mime)?;
    Some(AssistantContent::image_base64(data, Some(media_type), None))
}

/// Extracts the tool calls from a completed choice.
pub(super) fn tool_calls_of(
    choice: &[AssistantContent],
    names: &ToolNameMap,
) -> Vec<super::ModelToolCall> {
    choice
        .iter()
        .filter_map(|content| match content {
            AssistantContent::ToolCall(call) => {
                let wire = WireToolName::new(call.function.name.to_string());
                let name = names
                    .decode(&wire)
                    .map(|canonical| canonical.as_str().to_owned())
                    .unwrap_or_else(|| call.function.name.to_string());
                Some(super::ModelToolCall {
                    id: call.id.to_string(),
                    name,
                    arguments: serde_json::Value::Object(call.function.arguments.clone()).to_string(),
                })
            }
            _ => None,
        })
        .collect()
}
