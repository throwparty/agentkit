use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::store::{SessionStore, StoreError};
use crate::types::{Message, PromptTurn, Session};

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub struct InMemorySessionStore {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
}

impl InMemorySessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemorySessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn create_session(&self, session: Session) -> Result<(), StoreError> {
        let mut sessions = self.sessions.write().await;
        if sessions.contains_key(&session.id) {
            return Err(StoreError::AlreadyExists {
                entity: "session",
                id: session.id.clone(),
            });
        }
        sessions.insert(session.id.clone(), session);
        Ok(())
    }

    async fn get_session(&self, id: &str) -> Result<Session, StoreError> {
        let sessions = self.sessions.read().await;
        sessions.get(id).cloned().map(|mut s| {
            s.prompt_turn_count = 0;
            s
        }).ok_or_else(|| StoreError::NotFound {
            entity: "session",
            id: id.to_string(),
        })
    }

    async fn list_sessions(&self) -> Result<Vec<Session>, StoreError> {
        let sessions = self.sessions.read().await;
        let mut result: Vec<Session> = sessions.values().cloned().collect();
        for s in &mut result {
            s.prompt_turn_count = 0;
        }
        result.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(result)
    }

    async fn close_session(&self, id: &str) -> Result<(), StoreError> {
        let mut sessions = self.sessions.write().await;
        let session = sessions.get_mut(id).ok_or_else(|| StoreError::NotFound {
            entity: "session",
            id: id.to_string(),
        })?;
        session.active = false;
        session.updated_at = now();
        Ok(())
    }

    async fn set_session_mode(&self, id: &str, mode: String) -> Result<(), StoreError> {
        let mut sessions = self.sessions.write().await;
        let session = sessions.get_mut(id).ok_or_else(|| StoreError::NotFound {
            entity: "session",
            id: id.to_string(),
        })?;
        session.mode = Some(mode);
        session.updated_at = now();
        Ok(())
    }

    async fn set_session_head(
        &self,
        id: &str,
        head_prompt_turn_id: &str,
    ) -> Result<(), StoreError> {
        let mut sessions = self.sessions.write().await;
        let session = sessions.get_mut(id).ok_or_else(|| StoreError::NotFound {
            entity: "session",
            id: id.to_string(),
        })?;
        session.head_prompt_turn_id = Some(head_prompt_turn_id.to_string());
        session.updated_at = now();
        Ok(())
    }

    async fn append_prompt_turn(&self, _turn: PromptTurn) -> Result<(), StoreError> {
        Err(StoreError::Database("not implemented".to_string()))
    }

    async fn get_prompt_turn_children(
        &self,
        _id: &str,
    ) -> Result<Vec<PromptTurn>, StoreError> {
        Err(StoreError::Database("not implemented".to_string()))
    }

    async fn get_session_prompt_turns(
        &self,
        _session_id: &str,
    ) -> Result<Vec<PromptTurn>, StoreError> {
        Err(StoreError::Database("not implemented".to_string()))
    }

    async fn append_message(&self, _message: Message) -> Result<(), StoreError> {
        Err(StoreError::Database("not implemented".to_string()))
    }

    async fn get_messages_for_turn(&self, _turn_id: &str) -> Result<Vec<Message>, StoreError> {
        Err(StoreError::Database("not implemented".to_string()))
    }

    async fn get_context(
        &self,
        _session_id: &str,
        _max_turns: Option<usize>,
    ) -> Result<Vec<Message>, StoreError> {
        Err(StoreError::Database("not implemented".to_string()))
    }

    async fn fork_session(
        &self,
        _new_session: Session,
        _source_session_id: &str,
        _fork_point_turn_id: &str,
    ) -> Result<(), StoreError> {
        Err(StoreError::Database("not implemented".to_string()))
    }

    async fn clear(&self) -> Result<(), StoreError> {
        let mut sessions = self.sessions.write().await;
        sessions.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::run_session_tests;

    #[tokio::test]
    async fn test_session_crud() {
        let store = InMemorySessionStore::new();
        run_session_tests(&store).await;
    }
}
