//! Tackle: ACP-native agentic coding harness.

pub mod acp_v1;
pub mod agent;
pub mod builtins;
pub mod cli;
pub mod config;
pub mod credentials;
pub mod definitions;
pub mod invokables;
pub mod mcp;
pub mod permissions;
pub mod providers;
pub mod scripts;
pub mod store;
pub mod telemetry;

/// Re-exported for integration tests and downstream consumers.
pub use agent_client_protocol;
