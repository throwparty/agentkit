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

/// The invokable category implied by a namespaced name — the one place
/// the prefix convention is decoded. The categories are `mcp`, `prompt`,
/// `client`, and `agent`; `agent` is the fallback (actor invokables are
/// `agent.<actor>`). The permission pipeline and the ACP dispatch both
/// classify through this, so the two never disagree.
pub fn kind_of(name: &str) -> &'static str {
    if name.starts_with("mcp.") {
        "mcp"
    } else if name.starts_with("prompt.") {
        "prompt"
    } else if name.starts_with("client.") {
        "client"
    } else {
        "agent"
    }
}
