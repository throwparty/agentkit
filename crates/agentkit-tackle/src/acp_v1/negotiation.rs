//! Per-connection negotiation and client-surface state: the advertised unstable features, the client capability surface, the editor toggles, tracked terminals, and the connection's MCP pool.

use super::capabilities::UnstableFeature;
use super::client_tools;
use crate::mcp::McpPool;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

/// Per-connection negotiation and client-surface state, captured or
/// seeded when the connection opens and consulted by every update sender.
pub struct ConnectionNegotiation {
    supported: Mutex<BTreeSet<UnstableFeature>>,
    /// The client's advertised ACP capabilities (fs/terminal surface),
    /// the source of truth for which `client.*` tools exist.
    caps: Mutex<client_tools::ClientCaps>,
    /// The `/editor` toggles for the client-capability categories,
    /// seeded from `[client_tools]` and overridable at runtime.
    pub(super) editor: Mutex<client_tools::EditorToggles>,
    /// Terminal ids the harness created for this connection, by session,
    /// for release at session close. Connection-scoped and ephemeral.
    pub(super) terminals: Mutex<BTreeMap<String, BTreeSet<String>>>,
    /// The connection's MCP pool: the configured servers plus any the
    /// client forwarded, mutable via `/mcp`. Per-connection, so an
    /// availability toggle or a forwarded server never bleeds into
    /// another connection in the same process.
    mcp_pool: Arc<tokio::sync::Mutex<McpPool>>,
}

impl ConnectionNegotiation {
    /// A fresh connection's state, with the editor toggles seeded from
    /// configuration and its own MCP pool.
    pub fn new(editor: client_tools::EditorToggles, mcp_pool: McpPool) -> Self {
        Self {
            supported: Mutex::new(BTreeSet::new()),
            caps: Mutex::new(client_tools::ClientCaps::default()),
            editor: Mutex::new(editor),
            terminals: Mutex::new(BTreeMap::new()),
            mcp_pool: Arc::new(tokio::sync::Mutex::new(mcp_pool)),
        }
    }

    /// The connection's MCP pool handle.
    pub fn mcp_pool(&self) -> Arc<tokio::sync::Mutex<McpPool>> {
        Arc::clone(&self.mcp_pool)
    }

    pub(super) fn capture(
        &self,
        capabilities: &agent_client_protocol::schema::v1::ClientCapabilities,
    ) {
        *self.supported.lock().expect("negotiation poisoned") =
            UnstableFeature::advertised_in(&capabilities.meta);
        *self.caps.lock().expect("negotiation poisoned") =
            client_tools::ClientCaps::from_capabilities(capabilities);
    }

    /// Whether the client advertised support for `feature`.
    pub fn supports(&self, feature: UnstableFeature) -> bool {
        self.supported
            .lock()
            .expect("negotiation poisoned")
            .contains(&feature)
    }

    /// The client's advertised capability surface.
    pub fn client_caps(&self) -> client_tools::ClientCaps {
        *self.caps.lock().expect("negotiation poisoned")
    }

    /// The editor toggles currently in effect for this connection.
    pub(super) fn editor_toggles(&self) -> client_tools::EditorToggles {
        *self.editor.lock().expect("negotiation poisoned")
    }

    /// Records a terminal created for `session`, or drops one that has
    /// finished, so session close can release whatever remains.
    pub(super) fn track_terminals(
        &self,
        session_id: &str,
        created: Option<String>,
        finished: Option<String>,
    ) {
        if created.is_none() && finished.is_none() {
            return;
        }
        let mut terminals = self.terminals.lock().expect("negotiation poisoned");
        if let Some(id) = created {
            terminals
                .entry(session_id.to_owned())
                .or_default()
                .insert(id);
        }
        if let Some(id) = finished {
            if let Some(set) = terminals.get_mut(session_id) {
                set.remove(&id);
            }
        }
    }
}

impl crate::invokables::client_tools::TerminalTracker for ConnectionNegotiation {
    fn track(
        &self,
        session: &crate::store::SessionId,
        created: Option<String>,
        finished: Option<String>,
    ) {
        self.track_terminals(session, created, finished);
    }
}
