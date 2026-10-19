//! Invokable namespacing: the `mcp.`/`prompt.`/`agent.`/`client.` prefixes and the category they imply.

/// `mcp.<server>.<tool>`.
pub fn mcp_name(server: &str, tool: &str) -> String {
    format!("mcp.{server}.{tool}")
}

/// `prompt.<name>`.
pub fn prompt_name(name: &str) -> String {
    format!("prompt.{name}")
}

/// `agent.<name>`.
pub fn agent_name(name: &str) -> String {
    format!("agent.{name}")
}

/// The invokable categories, decoded from the namespaced name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Mcp,
    Prompt,
    Client,
    Agent,
}

impl Kind {
    /// The category name the policy-script API and the permission
    /// pipeline carry.
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Mcp => "mcp",
            Kind::Prompt => "prompt",
            Kind::Client => "client",
            Kind::Agent => "agent",
        }
    }
}

/// The invokable category implied by a namespaced name — the one place
/// the prefix convention is decoded. `None` when the name carries no
/// known prefix: an unknown invokable, never silently an agent. The
/// permission pipeline and the ACP dispatch both classify through this,
/// so the two never disagree.
pub fn kind_of(name: &str) -> Option<Kind> {
    if name.starts_with("mcp.") {
        Some(Kind::Mcp)
    } else if name.starts_with("prompt.") {
        Some(Kind::Prompt)
    } else if name.starts_with("client.") {
        Some(Kind::Client)
    } else if name.starts_with("agent.") {
        Some(Kind::Agent)
    } else {
        None
    }
}
