//! The model provider abstraction: the tackle-owned `ModelProvider` trait and the model-seam types (chat messages, tool definitions, requests, responses, errors) that cross it. Implementations live in per-wire-format submodules.

mod rig;
pub mod openai_chat_completions;

use std::future::Future;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatRole {
    #[default]
    User,
    Assistant,
    /// A tool-result message answering an assistant tool call.
    Tool,
}

/// One assembled context message. Carries text plus, for assistant
/// messages, the tool calls they requested, and, for `Tool` messages,
/// the result answering one.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
    pub tool_calls: Vec<ModelToolCall>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
}

impl ChatMessage {
    #[must_use]
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: ChatRole::User,
            text: text.into(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: ChatRole::Assistant,
            text: text.into(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn tool_result(
        call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            role: ChatRole::Tool,
            text: content.into(),
            tool_call_id: Some(call_id.into()),
            tool_name: Some(name.into()),
            ..Self::default()
        }
    }
}

/// A tool call the model requested.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// A tool offered to the model, in the registry's namespaced form.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// The result of executing one tool call: the content fed back to the
/// model, and whether it failed.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallOutcome {
    pub content: String,
    pub is_error: bool,
}

/// A tool executor that refuses every call: for loops that offer no tools
/// (tests, and turns where tools are unused).
#[cfg(test)]
#[must_use]
pub(crate) fn ignore_tool_call(
    _call: ModelToolCall,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolCallOutcome> + Send>> {
    Box::pin(async move {
        ToolCallOutcome {
            content: String::new(),
            is_error: true,
        }
    })
}

/// A completion request against a configured endpoint: `model` is the
/// endpoint-qualified address; the provider owns the bare wire name,
/// resolved at its construction — the request never overrides it.
#[derive(Debug, Clone)]
pub struct ModelRequest {
    pub model: String,
    pub system: String,
    /// The assembled context; the LAST message is the prompt.
    pub messages: Vec<ChatMessage>,
    /// Model-invokable tools offered to the model.
    pub tools: Vec<ToolDefinition>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ModelUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelResponse {
    /// The assistant's text content, concatenated.
    pub text: String,
    /// The tool calls the model requested (empty for a final answer).
    pub tool_calls: Vec<ModelToolCall>,
    pub usage: ModelUsage,
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("model `{model}` is not configured")]
    UnknownModel { model: String },
    #[error("credential resolution failed for `{identity}`")]
    Credential { identity: String },
    #[error("completion failed: {0}")]
    Completion(String),
    /// The stream failed AFTER text was already emitted to the client:
    /// the partial text rides the error so the caller can persist it
    /// (retrying would duplicate the deltas the client already saw).
    #[error("stream failed after {text_len} characters: {error}")]
    StreamFailed {
        text: String,
        text_len: usize,
        error: String,
    },
    #[error("malformed assembled context: {0}")]
    Context(String),
}

/// The model seam: tackle owns this trait; rig-core hides behind it.
pub trait ModelProvider: Send + Sync {
    /// Streaming completion: `on_text_delta` fires per text delta and
    /// `on_thought_delta` per reasoning/thought delta as they arrive (the
    /// caller relays chunks to the client live); the aggregated response
    /// (full text + usage) returns at the end. Thought deltas are advisory
    /// and never enter the persisted assistant message.
    fn stream_completion(
        &self,
        request: ModelRequest,
        on_text_delta: &mut (dyn FnMut(&str) + Send),
        on_thought_delta: &mut (dyn FnMut(&str) + Send),
    ) -> impl Future<Output = Result<ModelResponse, ModelError>> + Send;
}
