//! The in-memory backend: a plain Rust implementation of the store
//! semantics — the readable reference the SQLite queries are tested
//! against. Not durable; the test backend.

use super::{ListFilter, Message, Session, SessionId, SessionKind, Turn, TurnId, TurnUsage};
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
            session.updated_at = super::unix_now();
        }
    }

    pub fn delete_session(&mut self, id: &SessionId) {
        if let Some(session) = self.sessions.get_mut(id) {
            session.active = false;
            session.updated_at = super::unix_now();
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
}
