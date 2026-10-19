//! The in-memory, session-scoped grant store: standing allow/deny decisions. Never persisted.

use std::collections::BTreeMap;

/// The in-memory grant store, keyed by session, actor, and invokable:
/// an "allow for this session" grant must not outlive its session or
/// leak into another one in the same process.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct GrantStore {
    grants: BTreeMap<(String, String, String), GrantRecord>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct GrantRecord {
    pub(super) kind: GrantKind,
    pub(super) reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GrantKind {
    Allow,
    Deny,
}

impl GrantStore {
    pub fn allow_always(&mut self, session: &str, actor: &str, invokable: &str) {
        self.grants.insert(
            (session.to_owned(), actor.to_owned(), invokable.to_owned()),
            GrantRecord {
                kind: GrantKind::Allow,
                reason: None,
            },
        );
    }

    pub fn deny_always(
        &mut self,
        session: &str,
        actor: &str,
        invokable: &str,
        reason: Option<String>,
    ) {
        self.grants.insert(
            (session.to_owned(), actor.to_owned(), invokable.to_owned()),
            GrantRecord {
                kind: GrantKind::Deny,
                reason,
            },
        );
    }

    pub(super) fn held(&self, session: &str, actor: &str, invokable: &str) -> Option<&GrantRecord> {
        self.grants
            .get(&(session.to_owned(), actor.to_owned(), invokable.to_owned()))
    }

    /// Whether no grant records exist — elicitation responses and other
    /// non-pipeline paths must never create any.
    pub fn is_empty(&self) -> bool {
        self.grants.is_empty()
    }

    /// Drops every grant belonging to `session`: its active life is over.
    /// Other sessions in the same process are untouched.
    pub fn clear_session(&mut self, session: &str) {
        self.grants
            .retain(|(grant_session, _, _), _| grant_session != session);
    }

    /// The store's active life is over: close, process exit, and
    /// resume after restart all start from an empty store.
    pub fn clear(&mut self) {
        self.grants.clear();
    }
}
