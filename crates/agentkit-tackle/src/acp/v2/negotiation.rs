//! Per-connection v2 state: the cancellation registry and the MCP pool. v2
//! carries no client filesystem/terminal surface, so there is no capability
//! capture here and the terminal tracker is inert.

use crate::acp::common::cancellation::{CancelRegistration, CancellationRegistry};
use crate::invokables::client_tools::TerminalTracker;
use crate::mcp::McpPool;
use crate::store::SessionId;
use agent_client_protocol::schema::v2::AvailableCommand;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Per-connection v2 state, captured when the connection opens.
pub struct ConnectionNegotiation {
    cancellations: Arc<CancellationRegistry>,
    mcp_pool: Arc<tokio::sync::Mutex<McpPool>>,
    /// The rendered command list, cached until the pool changes.
    commands: tokio::sync::Mutex<Option<Vec<AvailableCommand>>>,
}

impl ConnectionNegotiation {
    /// A fresh connection's state.
    pub fn new() -> Self {
        Self::with_pool(Arc::new(tokio::sync::Mutex::new(McpPool::new())))
    }

    /// A connection's state over an existing pool, so configured servers are
    /// shared with the other protocol version on the same transport.
    pub fn with_pool(mcp_pool: Arc<tokio::sync::Mutex<McpPool>>) -> Self {
        Self {
            cancellations: Arc::new(CancellationRegistry::new()),
            mcp_pool,
            commands: tokio::sync::Mutex::new(None),
        }
    }

    /// The connection's MCP pool handle.
    pub fn mcp_pool(&self) -> Arc<tokio::sync::Mutex<McpPool>> {
        Arc::clone(&self.mcp_pool)
    }

    /// The cached command advertisement, if the pool has not changed since
    /// it was rendered.
    pub(super) async fn cached_commands(&self) -> Option<Vec<AvailableCommand>> {
        self.commands.lock().await.clone()
    }

    /// Caches a rendered command advertisement for this connection.
    pub(super) async fn cache_commands(&self, commands: Vec<AvailableCommand>) {
        *self.commands.lock().await = Some(commands);
    }

    /// Drops the cached advertisement after a pool mutation.
    pub(super) async fn invalidate_commands(&self) {
        *self.commands.lock().await = None;
    }

    /// Registers a fresh cancellation token for a turn in flight on
    /// `session`. The returned registration removes the entry when dropped.
    pub(super) fn register_cancellation(
        self: &Arc<Self>,
        session: &str,
    ) -> (CancellationToken, CancelRegistration) {
        self.cancellations.register(session)
    }

    /// Waits (briefly) for a cancelled turn to finish emitting its terminal
    /// idle, so `session/close` can order its response after it.
    pub(super) async fn wait_cancelled(&self, session: &str) {
        self.cancellations.wait_cancelled(session).await;
    }

    /// Cancels the turn in flight on `session`, if any.
    pub(super) fn cancel(&self, session: &str) {
        self.cancellations.cancel(session);
    }
}

impl Default for ConnectionNegotiation {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalTracker for ConnectionNegotiation {
    fn track(&self, _session: &SessionId, _created: Option<String>, _finished: Option<String>) {}
}

