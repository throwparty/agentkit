//! Shared rig-core plumbing: conversions between tackle model types and rig types.

use super::{ChatContent, ChatMessage, ChatRole};
use rig_core::message::{AssistantContent, ImageMediaType, Message, MimeType, UserContent};

pub(super) fn to_rig_message(message: &ChatMessage) -> Message {
    match message.role {
        ChatRole::User => Message::User {
            content: message
                .content
                .iter()
                .map(|content| match content {
                    ChatContent::Text(text) => UserContent::text(text.clone()),
                    ChatContent::Image { url } => UserContent::image_url(url.clone(), None, None),
                })
                .collect(),
        },
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
                content.push(AssistantContent::tool_call(
                    call.id.clone(),
                    call.name.clone(),
                    arguments,
                ));
            }
            Message::Assistant { id: None, content }
        }
        ChatRole::Tool => Message::tool_result(
            message.tool_call_id.clone().unwrap_or_default(),
            message.tool_name.clone().unwrap_or_default(),
            message.text(),
        ),
    }
}

/// Assistant image history only carries data URLs; rig's assistant image
/// constructor accepts base64 plus a known media type, not arbitrary URLs.
fn assistant_image(url: &str) -> Option<AssistantContent> {
    let (mime, data) = url.strip_prefix("data:")?.split_once(";base64,")?;
    let media_type = ImageMediaType::from_mime_type(mime)?;
    Some(AssistantContent::image_base64(data, Some(media_type), None))
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
