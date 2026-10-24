//! Per-connection negotiation and client-surface state: the advertised unstable features, the client capability surface, the editor toggles, tracked terminals, and the connection's MCP pool.

use super::capabilities::UnstableFeature;
use super::client_tools;
use crate::mcp::McpPool;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

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
    /// Cancellation tokens for in-flight prompt turns, by session, for
    /// `session/cancel`, `session/close`, and `$/cancel_request` to reach.
    /// Connection-scoped and ephemeral: a turn task is spawned on the
    /// connection, so it cannot outlive it.
    pub(super) cancellations: Mutex<BTreeMap<String, CancellationEntry>>,
    /// Monotonic id source for cancellation registrations, so a guard
    /// removes only the entry it created and never a later turn's.
    cancellation_seq: AtomicU64,
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
            cancellations: Mutex::new(BTreeMap::new()),
            cancellation_seq: AtomicU64::new(0),
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

impl ConnectionNegotiation {
    /// Registers a fresh cancellation token for a turn in flight on
    /// `session`. The returned [`CancelRegistration`] removes the entry
    /// when dropped, so every exit path (completion, cancellation, error,
    /// connection drop) cleans up.
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

    /// Cancels the turn in flight on `session`, if any. A session with no
    /// in-flight turn is a no-op.
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

/// A registered cancellation: the token, plus the id that identifies this
/// registration so its guard removes only its own entry. A later turn's
/// registration for the same session must never be deleted by an earlier
/// turn's guard.
pub(super) struct CancellationEntry {
    token: CancellationToken,
    id: u64,
}

/// Removes a turn's cancellation registration when the turn ends, but only
/// if the registry still holds *this* registration.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp_v1::client_tools::EditorToggles;

    fn negotiation() -> Arc<ConnectionNegotiation> {
        Arc::new(ConnectionNegotiation::new(
            EditorToggles {
                fs: true,
                terminal: true,
            },
            McpPool::new(),
        ))
    }

    #[test]
    fn cancel_triggers_the_registered_token() {
        let negotiation = negotiation();
        let (token, _registration) = negotiation.register_cancellation("sess-1");
        assert!(!token.is_cancelled());
        negotiation.cancel("sess-1");
        assert!(token.is_cancelled());
    }

    #[test]
    fn cancel_for_an_unregistered_session_is_a_noop() {
        let negotiation = negotiation();
        let (token, _registration) = negotiation.register_cancellation("sess-1");
        negotiation.cancel("other");
        assert!(!token.is_cancelled());
    }

    #[test]
    fn dropping_the_registration_removes_the_token() {
        let negotiation = negotiation();
        let (token, registration) = negotiation.register_cancellation("sess-1");
        drop(registration);
        // The entry is gone, so a later cancel does not reach the token.
        negotiation.cancel("sess-1");
        assert!(!token.is_cancelled());
    }

    #[test]
    fn cancellation_is_idempotent() {
        let negotiation = negotiation();
        let (token, _registration) = negotiation.register_cancellation("sess-1");
        negotiation.cancel("sess-1");
        negotiation.cancel("sess-1");
        assert!(token.is_cancelled());
    }

    #[test]
    fn a_stale_registration_does_not_remove_a_newer_one() {
        let negotiation = negotiation();
        // Turn A registers; turn B replaces the entry while A is still
        // finalising (its lease released, its guard not yet dropped).
        let (old_token, old_registration) = negotiation.register_cancellation("sess-1");
        let (new_token, _new_registration) = negotiation.register_cancellation("sess-1");
        drop(old_registration);
        // A's guard must not have removed B's entry.
        negotiation.cancel("sess-1");
        assert!(new_token.is_cancelled(), "the newer registration survives");
        let _ = old_token;
    }
}
