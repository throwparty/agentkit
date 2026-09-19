//! The agent loop's model seam: a tackle-owned `ModelProvider` trait with
//! the rig-core implementation behind it, so pre-1.0 churn is a
//! single-module change and tests can substitute doubles.

pub mod provider;

use std::future::Future;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
}

/// One assembled context message. Text-only at this seam: the content
/// blocks' text is extracted by the caller (tool calls ride the loop in
/// T-015, not the request type).
#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
}

/// A completion request against a configured endpoint: the model name is
/// bare (endpoint-qualified names resolve at construction).
#[derive(Debug, Clone)]
pub struct ModelRequest {
    pub model: String,
    pub system: String,
    /// The assembled context; the LAST message is the prompt.
    pub messages: Vec<ChatMessage>,
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
    pub usage: ModelUsage,
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("unsupported wire format for endpoint `{endpoint}`: {format:?}")]
    UnsupportedWireFormat {
        endpoint: String,
        format: crate::config::WireFormat,
    },
    #[error("model `{model}` is not configured")]
    UnknownModel { model: String },
    #[error("credential resolution failed for `{identity}`")]
    Credential { identity: String },
    #[error("completion failed: {0}")]
    Completion(String),
    #[error("malformed assembled context: {0}")]
    Context(String),
}

/// The model seam: tackle owns this trait; rig-core hides behind it.
pub trait ModelProvider: Send + Sync {
    fn complete(
        &self,
        request: ModelRequest,
    ) -> impl Future<Output = Result<ModelResponse, ModelError>> + Send;
}
