//! The in-memory backend: a plain Rust implementation of the store
//! semantics — the readable reference the SQLite queries are tested
//! against. Not durable; the test backend.

use super::{
    ListFilter, Message, Session, SessionId, SessionKind, Turn, TurnId, TurnKind, TurnUsage,
};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct MemoryData {
    sessions: BTreeMap<SessionId, Session>,
    turns: BTreeMap<TurnId, Turn>,
    /// Messages by turn, position-ordered by insertion.
    messages: BTreeMap<TurnId, Vec<Message>>,
}

impl MemoryData {
    pub fn create_session(&mut self, session: Session) {
        self.sessions.insert(session.id.clone(), session);
    }

    pub fn get_session(&self, id: &SessionId) -> Option<Session> {
        self.sessions.get(id).cloned()
    }

    pub fn list_sessions(&self, filter: &ListFilter<'_>) -> Vec<Session> {
        let mut matches: Vec<Session> = self
            .sessions
            .values()
            .filter(|session| session.active || filter.include_deleted)
            .filter(|session| filter.include_ephemeral || session.kind == SessionKind::Interactive)
            .filter(|session| filter.cwd.is_none_or(|cwd| session.cwd == cwd))
            .cloned()
            .collect();
        matches.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        matches
    }

    pub fn close_session(&mut self, id: &SessionId) {
        if let Some(session) = self.sessions.get_mut(id) {
            session.owner = None;
            session.lease_expires_at = None;
            session.updated_at = super::unix_now();
        }
    }

    /// Attempts to acquire the lease; `expires_at` is the absolute expiry
    /// (unix seconds). Succeeds when free or expired.
    pub fn acquire_lease(
        &mut self,
        session_id: &SessionId,
        owner: &str,
        expires_at: i64,
        now: i64,
    ) -> bool {
        match self.sessions.get_mut(session_id) {
            Some(session) => {
                let free = session.owner.is_none()
                    || session
                        .lease_expires_at
                        .is_some_and(|expires| expires < now);
                if free {
                    session.owner = Some(owner.to_owned());
                    session.lease_expires_at = Some(expires_at);
                    true
                } else {
                    false
                }
            }
            None => false,
        }
    }

    /// Refreshes the lease; false if another connection holds it.
    pub fn heartbeat_lease(
        &mut self,
        session_id: &SessionId,
        owner: &str,
        expires_at: i64,
    ) -> bool {
        match self.sessions.get_mut(session_id) {
            Some(session) if session.owner.as_deref() == Some(owner) => {
                session.lease_expires_at = Some(expires_at);
                true
            }
            _ => false,
        }
    }

    /// The live lease holder, if any.
    pub fn lease_holder(&self, session_id: &SessionId, now: i64) -> Option<String> {
        self.sessions
            .get(session_id)
            .filter(|session| session.owner.is_some())
            .filter(|session| {
                session
                    .lease_expires_at
                    .is_some_and(|expires| expires >= now)
            })
            .map(|session| session.owner.clone().expect("owner checked above"))
    }

    /// Soft-deletes; refused while the session is actively leased.
    pub fn delete_session(&mut self, id: &SessionId, now: i64) -> Result<(), super::StoreError> {
        match self.sessions.get_mut(id) {
            Some(session) => {
                if session.owner.is_some()
                    && session
                        .lease_expires_at
                        .is_some_and(|expires| expires >= now)
                {
                    return Err(super::StoreError::LeaseHeld {
                        session: id.clone(),
                        owner: session.owner.clone().expect("owner checked above"),
                    });
                }
                session.active = false;
                session.updated_at = now;
                Ok(())
            }
            None => Ok(()),
        }
    }

    pub fn append_turn(&mut self, turn: Turn) {
        if let Some(session) = self.sessions.get_mut(&turn.session_id) {
            session.head_turn_id = Some(turn.id.clone());
            session.updated_at = turn.created_at;
        }
        self.turns.insert(turn.id.clone(), turn);
    }

    pub fn append_message(&mut self, mut message: Message) -> Message {
        let turn_messages = self.messages.entry(message.turn_id.clone()).or_default();
        message.position = turn_messages.len() as i64;
        turn_messages.push(message.clone());
        message
    }

    pub fn session_usage(&self, session_id: &SessionId) -> TurnUsage {
        self.turns
            .values()
            .filter(|turn| turn.session_id == *session_id)
            .map(|turn| turn.usage)
            .fold(TurnUsage::default(), TurnUsage::add)
    }

    pub fn set_turn_usage(&mut self, turn_id: &TurnId, usage: TurnUsage) {
        if let Some(turn) = self.turns.get_mut(turn_id) {
            turn.usage = usage;
        }
    }

    /// The parent-chain walk from the head with the compaction truncation
    /// state machine — the Rust mirror of the SQL CTE.
    fn context_walk(&self, session_id: &SessionId) -> Vec<(TurnId, TurnKind)> {
        let session = match self.sessions.get(session_id) {
            Some(session) => session,
            None => return Vec::new(),
        };
        let mut walk = Vec::new();
        let mut stop_at: Option<TurnId> = None;
        let mut current = session.head_turn_id.clone();

        while let Some(id) = current {
            let Some(turn) = self.turns.get(&id) else {
                break; // dangling parent: defensive stop
            };
            walk.push((id.clone(), turn.kind));

            if turn.kind == TurnKind::Compaction {
                match &turn.first_retained_turn_id {
                    None => break, // full compaction
                    Some(retained) => {
                        stop_at = Some(retained.clone());
                        current = turn.parent_id.clone();
                    }
                }
            } else if stop_at.as_ref() == Some(&id) {
                break; // keep-recent boundary
            } else {
                current = turn.parent_id.clone();
            }
        }

        walk
    }

    pub fn assemble_context(&self, session_id: &SessionId) -> Vec<super::AssembledMessage> {
        let order = super::assembly_order(self.context_walk(session_id));
        order
            .iter()
            .flat_map(|turn_id| {
                self.messages
                    .get(turn_id)
                    .into_iter()
                    .flatten()
                    .map(|message| super::AssembledMessage {
                        message: message.clone(),
                        turn_kind: self.turns[turn_id].kind,
                    })
            })
            .collect()
    }
}
