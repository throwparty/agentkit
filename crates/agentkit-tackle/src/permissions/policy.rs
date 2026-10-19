//! The policy-script decision source and its payload: `pre_tool_use` returns allow, deny, or ask.

use serde_json::Value;


/// The policy-script decision source: implemented by the Rhai policy
/// host. The pipeline treats an absent function, malformed
/// return, or script error — any `None` — as fail-closed to ask.
pub trait PolicyScript: Send + Sync {
    fn pre_tool_use(&self, request: &PolicyRequest) -> Option<PolicyVerdict>;
}

/// The `pre_tool_use(request)` payload, per scripts-api.md.
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyRequest {
    /// The namespaced invokable: `mcp.<server>.<tool>`, `prompt.<name>`,
    /// or `agent.<name>`.
    pub invokable: String,
    /// `mcp` | `prompt` | `agent`.
    pub kind: &'static str,
    /// Structured, named arguments.
    pub arguments: Value,
    /// Whether the session is currently user-attended. Informs scripts
    /// only; the pipeline never branches on it.
    pub interactive: bool,
    /// The grant store held an allow record for this invokable.
    pub previously_granted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PolicyVerdict {
    pub decision: Decision,
    /// Surfaces in the permission prompt content on ask.
    pub reason: Option<String>,
}
