//! The connection-provided collaborators a prompt turn runs against: the
//! MCP pool, the outbound sink, the permission requester, the client-tool
//! executor and terminal tracker, the notice sinks, and the client-tool
//! catalogue. Bundled so the orchestration functions stay ACP-free and
//! their signatures stay readable.

use crate::agent::sink::TurnSink;
use crate::invokables::client_tools::{ClientTool, ClientToolExecutor, TerminalTracker};
use crate::mcp::McpPool;
use crate::permissions::PermissionRequester;
use crate::scripts::host::NoticeSink;
use std::sync::Arc;

/// The runtime a turn is driven with. Constructed by the RPC layer from
/// the live connection and passed into the orchestration.
pub struct TurnRuntime {
    pub pool: Arc<tokio::sync::Mutex<McpPool>>,
    pub sink: Arc<dyn TurnSink>,
    pub requester: Arc<dyn PermissionRequester>,
    pub executor: Arc<dyn ClientToolExecutor>,
    pub tracker: Arc<dyn TerminalTracker>,
    /// Notices for the detached post-turn compaction firing.
    pub compaction_notices: Arc<dyn NoticeSink>,
    /// Notices for the detached titling firing.
    pub titling_notices: Arc<dyn NoticeSink>,
    /// The client-capability tools the connection advertised and enabled.
    pub client_tools: Vec<ClientTool>,
}
