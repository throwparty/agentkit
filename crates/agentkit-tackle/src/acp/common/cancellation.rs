//! The per-connection cancellation registry, shared by both protocol
//! versions. A prompt turn registers a token; `session/cancel`,
//! `session/close`, and `$/cancel_request` cancel it; the guard removes the
//! entry when the turn ends, but only if it still owns that registration.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// Cancellation tokens for in-flight prompt turns, by session.
pub(crate) struct CancellationRegistry {
    entries: Mutex<BTreeMap<String, CancellationEntry>>,
    /// Monotonic id source so a guard removes only the entry it created and
    /// never a later turn's.
    seq: AtomicU64,
}

impl CancellationRegistry {
    pub(crate) fn new() -> Self {
        Self {
            entries: Mutex::new(BTreeMap::new()),
            seq: AtomicU64::new(0),
        }
    }

    /// Registers a fresh token for a turn in flight on `session`. The
    /// returned guard removes the entry when dropped.
    pub(crate) fn register(
        self: &Arc<Self>,
        session: &str,
    ) -> (CancellationToken, CancelRegistration) {
        let token = CancellationToken::new();
        let id = self.seq.fetch_add(1, Ordering::Relaxed);
        self.entries
            .lock()
            .expect("cancellation registry poisoned")
            .insert(
                session.to_owned(),
                CancellationEntry {
                    token: token.clone(),
                    id,
                },
            );
        let registration = CancelRegistration {
            registry: Arc::clone(self),
            session: session.to_owned(),
            id,
        };
        (token, registration)
    }

    /// Cancels the turn in flight on `session`, if any. A session with no
    /// in-flight turn is a no-op.
    pub(crate) fn cancel(&self, session: &str) {
        let token = self
            .entries
            .lock()
            .expect("cancellation registry poisoned")
            .get(session)
            .map(|entry| entry.token.clone());
        if let Some(token) = token {
            token.cancel();
        }
    }

    /// Waits (briefly) for a cancelled turn to finish emitting its terminal
    /// idle, so `session/close` can order its response after it.
    pub(crate) async fn wait_cancelled(&self, session: &str) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let present = self
                .entries
                .lock()
                .expect("cancellation registry poisoned")
                .contains_key(session);
            if !present || tokio::time::Instant::now() >= deadline {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }
}

impl Default for CancellationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

struct CancellationEntry {
    token: CancellationToken,
    id: u64,
}

/// Removes a turn's registration when the turn ends, but only if the
/// registry still holds this registration.
pub(crate) struct CancelRegistration {
    registry: Arc<CancellationRegistry>,
    session: String,
    id: u64,
}

impl Drop for CancelRegistration {
    fn drop(&mut self) {
        let mut entries = self
            .registry
            .entries
            .lock()
            .expect("cancellation registry poisoned");
        if entries.get(&self.session).map(|entry| entry.id) == Some(self.id) {
            entries.remove(&self.session);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Arc<CancellationRegistry> {
        Arc::new(CancellationRegistry::new())
    }

    #[test]
    fn cancel_triggers_the_registered_token() {
        let registry = registry();
        let (token, _registration) = registry.register("sess-1");
        assert!(!token.is_cancelled());
        registry.cancel("sess-1");
        assert!(token.is_cancelled());
    }

    #[test]
    fn cancel_for_an_unregistered_session_is_a_noop() {
        let registry = registry();
        let (token, _registration) = registry.register("sess-1");
        registry.cancel("other");
        assert!(!token.is_cancelled());
    }

    #[test]
    fn dropping_the_registration_removes_the_token() {
        let registry = registry();
        let (token, registration) = registry.register("sess-1");
        drop(registration);
        registry.cancel("sess-1");
        assert!(!token.is_cancelled());
    }

    #[test]
    fn cancellation_is_idempotent() {
        let registry = registry();
        let (token, _registration) = registry.register("sess-1");
        registry.cancel("sess-1");
        registry.cancel("sess-1");
        assert!(token.is_cancelled());
    }

    #[test]
    fn a_stale_registration_does_not_remove_a_newer_one() {
        let registry = registry();
        let (old_token, old_registration) = registry.register("sess-1");
        let (new_token, _new_registration) = registry.register("sess-1");
        drop(old_registration);
        registry.cancel("sess-1");
        assert!(new_token.is_cancelled(), "the newer registration survives");
        let _ = old_token;
    }
}
