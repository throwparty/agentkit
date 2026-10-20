//! The turn loop's outbound interface: the events the loop reports as a
//! turn runs. The core loop knows nothing of ACP; the RPC layer implements
//! this and maps each event to its wire notification.

use crate::invokables::titles::ToolKind;
use crate::store::SessionId;

/// The lifecycle status of a tool call, independent of the wire type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// A tool call announced to the client.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub title: String,
    pub name: String,
    pub kind: ToolKind,
    pub status: ToolCallStatus,
    pub arguments: serde_json::Value,
}

/// A terminal update to a previously announced tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallUpdate {
    pub id: String,
    pub status: ToolCallStatus,
    pub content: String,
}

/// Receives the loop's progress events. Implementations must not block: the
/// loop calls these inline while streaming.
pub trait TurnSink: Send + Sync {
    /// Per-model-request usage. `context_size` is the model's window; when
    /// unknown the caller skips the event entirely. `cost` is the
    /// cumulative session cost in USD, omitted when unknowable.
    fn usage(&self, session: &SessionId, input_tokens: u64, context_size: u64, cost: Option<f64>);

    /// A streamed assistant text delta, belonging to `message_id`.
    fn text_delta(&self, session: &SessionId, message_id: &str, text: &str);

    /// A streamed reasoning delta, belonging to `message_id` (advisory,
    /// never persisted).
    fn thought_delta(&self, session: &SessionId, message_id: &str, text: &str);

    /// A retry attempt, surfaced as a status card.
    fn retry(&self, session: &SessionId, attempt: u32, message: &str);

    /// A tool call the model requested, before execution.
    fn tool_call(&self, session: &SessionId, call: ToolCall);

    /// The terminal status and content of a tool call.
    fn tool_call_update(&self, session: &SessionId, update: ToolCallUpdate);

    /// The session's last-activity stamp, sent each turn before the
    /// response.
    fn session_info(&self, session: &SessionId, updated_at: &str);
}
