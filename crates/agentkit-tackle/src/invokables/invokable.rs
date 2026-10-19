//! The invokable: a namespaced, permission-gated operation with its visibility flags.

#[derive(Debug, Clone, PartialEq)]
pub enum Invokable {
    /// An MCP tool: user-invokable (direct invocation) and
    /// model-invokable.
    Mcp {
        name: String,
        server: String,
        tool: String,
        description: String,
        schema: serde_json::Value,
    },
    /// A reusable prompt: visibility from its frontmatter. `parameters`
    /// are positional (`{{ name }}` placeholders, the last taking the
    /// remainder of the arguments).
    Prompt {
        name: String,
        description: String,
        parameters: Vec<String>,
        body: String,
        compaction: bool,
        user_invokable: bool,
        model_invokable: bool,
    },
    /// An actor invokable: model-invokable only — nested agent loops
    /// spawn from model tool calls, never user slash commands.
    Agent { name: String, description: String },
    /// A client-capability tool (`client.<surface>.<method>`):
    /// model-invokable, and user-invokable via `/!` like an MCP tool.
    /// Executed on the connection against the ACP client rather than the
    /// MCP pool.
    Client {
        name: String,
        description: String,
        schema: serde_json::Value,
    },
}

impl Invokable {
    /// The namespaced invokable name.
    pub fn name(&self) -> &str {
        match self {
            Invokable::Mcp { name, .. }
            | Invokable::Prompt { name, .. }
            | Invokable::Agent { name, .. }
            | Invokable::Client { name, .. } => name,
        }
    }

    pub fn user_invokable(&self) -> bool {
        match self {
            Invokable::Mcp { .. } => true,
            Invokable::Prompt { user_invokable, .. } => *user_invokable,
            Invokable::Agent { .. } => false,
            Invokable::Client { .. } => true,
        }
    }

    pub fn model_invokable(&self) -> bool {
        match self {
            Invokable::Mcp { .. } => true,
            Invokable::Prompt {
                model_invokable, ..
            } => *model_invokable,
            Invokable::Agent { .. } => true,
            Invokable::Client { .. } => true,
        }
    }

    /// The invokable's human-readable description.
    pub fn description(&self) -> &str {
        match self {
            Invokable::Mcp { description, .. }
            | Invokable::Prompt { description, .. }
            | Invokable::Agent { description, .. }
            | Invokable::Client { description, .. } => description,
        }
    }
}
