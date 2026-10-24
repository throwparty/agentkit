//! Per-connection v2 state: the cancellation registry and the MCP pool. v2
//! carries no client filesystem/terminal surface, so there is no capability
//! capture here and the terminal tracker is inert.

use crate::invokables::client_tools::TerminalTracker;
use crate::mcp::McpPool;
use crate::store::SessionId;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// Per-connection v2 state, captured when the connection opens.
pub struct ConnectionNegotiation {
    cancellations: Mutex<BTreeMap<String, CancellationEntry>>,
    cancellation_seq: AtomicU64,
    mcp_pool: Arc<tokio::sync::Mutex<McpPool>>,
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
            cancellations: Mutex::new(BTreeMap::new()),
            cancellation_seq: AtomicU64::new(0),
            mcp_pool,
        }
    }

    /// The connection's MCP pool handle.
    pub fn mcp_pool(&self) -> Arc<tokio::sync::Mutex<McpPool>> {
        Arc::clone(&self.mcp_pool)
    }

    /// Registers a fresh cancellation token for a turn in flight on
    /// `session`. The returned registration removes the entry when dropped.
    pub(super) fn register_cancellation(
        self: &Arc<Self>,
        session: &str,
    ) -> (CancellationToken, CancelRegistration) {
        let token = CancellationToken::new();
        let id = self.cancellation_seq.fetch_add(1, Ordering::Relaxed);
        self.cancellations
            .lock()
            .expect("negotiation poisoned")
            .insert(
                session.to_owned(),
                CancellationEntry {
                    token: token.clone(),
                    id,
                },
            );
        let registration = CancelRegistration {
            negotiation: Arc::clone(self),
            session: session.to_owned(),
            id,
        };
        (token, registration)
    }

    /// Waits (briefly) for a cancelled turn to finish emitting its terminal
    /// idle, so `session/close` can order its response after it.
    pub(super) async fn wait_cancelled(&self, session: &str) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let present = self
                .cancellations
                .lock()
                .expect("negotiation poisoned")
                .contains_key(session);
            if !present || tokio::time::Instant::now() >= deadline {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    /// Cancels the turn in flight on `session`, if any.
    pub(super) fn cancel(&self, session: &str) {
        let token = self
            .cancellations
            .lock()
            .expect("negotiation poisoned")
            .get(session)
            .map(|entry| entry.token.clone());
        if let Some(token) = token {
            token.cancel();
        }
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

struct CancellationEntry {
    token: CancellationToken,
    id: u64,
}

/// Removes a turn's cancellation registration when the turn ends, but only if
/// the registry still holds this registration.
pub(super) struct CancelRegistration {
    negotiation: Arc<ConnectionNegotiation>,
    session: String,
    id: u64,
}

impl Drop for CancelRegistration {
    fn drop(&mut self) {
        let mut cancellations = self
            .negotiation
            .cancellations
            .lock()
            .expect("negotiation poisoned");
        if cancellations.get(&self.session).map(|entry| entry.id) == Some(self.id) {
            cancellations.remove(&self.session);
        }
    }
}
