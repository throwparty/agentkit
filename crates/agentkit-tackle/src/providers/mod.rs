//! The model provider abstraction: the tackle-owned `ModelProvider` trait
//! and the model-seam types (chat messages, tool definitions, requests,
//! responses, errors) that cross it. Implementations live in per-wire-format
//! submodules.
//!
//! This module is declarations and re-exports only.

mod rig;
mod types;

pub mod openai_chat_completions;

pub use types::{
    ChatMessage, ChatRole, ModelError, ModelProvider, ModelRequest, ModelResponse, ModelToolCall,
    ModelUsage, ToolCallOutcome, ToolDefinition,
};
#[cfg(test)]
pub(crate) use types::ignore_tool_call;
