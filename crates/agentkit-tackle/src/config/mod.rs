//! Layered configuration: a user layer and a project layer, merged with
//! project precedence.
//!
//! Security-relevant fields — model endpoints and the credential helper —
//! are user-configuration only: a project layer declaring them is rejected
//! with a named error, so a cloned repository cannot redirect model traffic
//! or credential resolution (Scout finding 2). Project `mcp_servers`
//! entries are accepted but trust-gated (see [`trust`]).

mod load;
mod schema;

pub mod trust;

#[cfg(test)]
mod tests;

pub use load::{
    load_layered, load_layered_deferred, project_trust_entries, resolve_script_source,
    resolve_script_sources, retain_project_entries,
};
pub use schema::{
    Auth, ClientToolsConfig, Config, ConfigError, Defaults, EndpointConfig, Loaded, McpServerConfig,
    ModelConfig, ModelsConfig, ScriptConfig, SessionsConfig, ToolNamesConfig, TurnsConfig,
    WireFormat,
};
