//! Tackle: ACP-native agentic coding harness.

pub mod acp;
pub mod agent;
pub mod builtins;
pub mod cli;
pub mod config;
pub mod invokables;
pub mod loader;
pub mod mcp;
pub mod permissions;
pub mod scripts;
pub mod store;
pub mod telemetry;

/// Re-exported for integration tests and downstream consumers.
pub use agent_client_protocol;
