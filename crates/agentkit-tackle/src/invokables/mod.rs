//! The unified invokable registry: every dispatchable operation is
//! namespaced — `mcp.<server>.<tool>`, `prompt.<name>`, `agent.<actor>` —
//! with user-invokable and model-invokable visibility. The registry is
//! the only dispatch point: the ACP command surface and the model's tool
//! exposure are views over it, and the harness ships no built-in tools
//! beyond it.
//!
//! Namespacing makes cross-mechanism collisions impossible. Within a
//! namespace, layer precedence (project over user) resolves at load; a
//! residual same-layer collision is a configuration error, never a
//! silent shadow.

pub mod client_tools;
pub mod titles;
pub mod tool_name;

mod direct;
mod invokable;
mod name;
mod registry;

#[cfg(test)]
mod tests;

pub use direct::{parse_direct, DirectError, DirectInvocation};
pub use invokable::Invokable;
pub use name::{agent_name, kind_of, mcp_name, prompt_name, Kind};
pub use registry::{ExpansionError, Registry, RegistryError};
pub use tool_name::{
    CohereToolNameScheme, CustomToolNameScheme, GeminiToolNameScheme, InvokableName,
    MetaToolNameScheme, NamespaceSeparator, StrictToolNameScheme, ToolNameError, ToolNameScheme,
    ToolNameSchemeName, WireToolName,
};
