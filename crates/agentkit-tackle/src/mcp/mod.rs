//! The MCP client pool: connections to configured and client-provided
//! servers, tools namespaced `mcp.<server>.<tool>`, per-server status.
//!
//! Connection is asynchronous with a per-server timeout, so session
//! creation never blocks; the status of every server (connected/failed)
//! is reported at the first turn. Nothing about which servers exist is
//! hardcoded.
//!
//! Spawn hygiene: stdio servers start with explicit argv (no shell
//! interpretation) and only the named environment entries — tackle's
//! own environment is never forwarded. A whole env value of the form
//! `{cred:IDENTITY}` resolves through the credential helper at spawn
//! (never at config load), so secrets never enter `Config`. Child
//! stderr is relayed to tackle's stderr line-prefixed with the server
//! name under a rate cap; the child's stdout is the MCP protocol
//! channel and is never touched by the relay.

mod pool;
mod spawn;
mod stderr;

#[cfg(test)]
mod tests;

pub use pool::{
    connect_forwarded, AutoDecline, Elicitation, ElicitationResponse, ElicitationSink, McpPool,
    McpPoolError, NamespacedTool, ServerStatus, ToolResult, TOOL_RESULT_LIMIT,
};
pub use spawn::{
    configure_stdio_spawn, credential_marker_identity, ConfigureCommand, EnvValueError,
};
pub use stderr::{
    relay_stderr, Admission, StderrRateCap, STDERR_RELAY_MAX_LINES, STDERR_RELAY_WINDOW,
};
