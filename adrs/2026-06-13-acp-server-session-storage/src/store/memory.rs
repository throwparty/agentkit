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
    prompt_turns: Arc<RwLock<HashMap<String, PromptTurn>>>,
    messages: Arc<RwLock<HashMap<String, Message>>>,
}

impl InMemorySessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            prompt_turns: Arc::new(RwLock::new(HashMap::new())),
            messages: Arc::new(RwLock::new(HashMap::new())),
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

    async fn append_prompt_turn(&self, turn: PromptTurn) -> Result<(), StoreError> {
        {
            let sessions = self.sessions.read().await;
            if !sessions.contains_key(&turn.session_id) {
                return Err(StoreError::NotFound {
                    entity: "session",
                    id: turn.session_id.clone(),
                });
            }
        }
        let mut prompt_turns = self.prompt_turns.write().await;
        prompt_turns.insert(turn.id.clone(), turn);
        Ok(())
    }

    async fn get_prompt_turn_children(
        &self,
        id: &str,
    ) -> Result<Vec<PromptTurn>, StoreError> {
        let prompt_turns = self.prompt_turns.read().await;
        let mut children: Vec<PromptTurn> = prompt_turns
            .values()
            .filter(|t| t.parent_id.as_deref() == Some(id))
            .cloned()
            .collect();
        children.sort_by(|a, b| a.position.cmp(&b.position));
        Ok(children)
    }

    async fn get_session_prompt_turns(
        &self,
        session_id: &str,
    ) -> Result<Vec<PromptTurn>, StoreError> {
        let prompt_turns = self.prompt_turns.read().await;
        let mut turns: Vec<PromptTurn> = prompt_turns
            .values()
            .filter(|t| t.session_id == session_id)
            .cloned()
            .collect();
        turns.sort_by(|a, b| a.position.cmp(&b.position));
        Ok(turns)
    }

    async fn append_message(&self, message: Message) -> Result<(), StoreError> {
        {
            let prompt_turns = self.prompt_turns.read().await;
            if !prompt_turns.contains_key(&message.prompt_turn_id) {
                return Err(StoreError::NotFound {
                    entity: "prompt_turn",
                    id: message.prompt_turn_id.clone(),
                });
            }
        }
        let mut messages = self.messages.write().await;
        messages.insert(message.id.clone(), message);
        Ok(())
    }

    async fn get_messages_for_turn(&self, turn_id: &str) -> Result<Vec<Message>, StoreError> {
        let messages = self.messages.read().await;
        let mut result: Vec<Message> = messages
            .values()
            .filter(|m| m.prompt_turn_id == turn_id)
            .cloned()
            .collect();
        result.sort_by(|a, b| a.position.cmp(&b.position));
        Ok(result)
    }

    async fn get_context(
        &self,
        session_id: &str,
        max_turns: Option<usize>,
    ) -> Result<Vec<Message>, StoreError> {
        {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).ok_or_else(|| StoreError::NotFound {
                entity: "session",
                id: session_id.to_string(),
            })?;
        }

        let turns = self.get_session_prompt_turns(session_id).await?;
        let turns: Vec<PromptTurn> = match max_turns {
            Some(0) => return Ok(vec![]),
            Some(n) => turns.into_iter().rev().take(n).rev().collect(),
            None => turns,
        };

        let mut result = Vec::new();
        for turn in &turns {
            result.append(&mut self.get_messages_for_turn(&turn.id).await?);
        }
        Ok(result)
    }

    async fn fork_session(
        &self,
        new_session: Session,
        source_session_id: &str,
        fork_point_turn_id: &str,
    ) -> Result<(), StoreError> {
        {
            let sessions = self.sessions.read().await;
            sessions.get(source_session_id).ok_or_else(|| StoreError::NotFound {
                entity: "session",
                id: source_session_id.to_string(),
            })?;
        }

        let source_turns = self.get_session_prompt_turns(source_session_id).await?;
        let fork_idx = source_turns
            .iter()
            .position(|t| t.id == fork_point_turn_id)
            .ok_or_else(|| StoreError::NotFound {
                entity: "prompt_turn",
                id: fork_point_turn_id.to_string(),
            })?;

        let mut turn_id_map: HashMap<String, String> = HashMap::new();

        {
            let mut prompt_turns = self.prompt_turns.write().await;
            for turn in &source_turns[..=fork_idx] {
                let new_turn_id = uuid::Uuid::new_v4().to_string();
                turn_id_map.insert(turn.id.clone(), new_turn_id.clone());
                let mut forked_turn = turn.clone();
                forked_turn.id = new_turn_id;
                forked_turn.session_id = new_session.id.clone();
                prompt_turns.insert(forked_turn.id.clone(), forked_turn);
            }
        }

        {
            let mut messages = self.messages.write().await;
            for (old_turn_id, new_turn_id) in &turn_id_map {
                let turn_msgs: Vec<Message> = messages
                    .values()
                    .filter(|m| &m.prompt_turn_id == old_turn_id)
                    .cloned()
                    .collect();
                for msg in turn_msgs {
                    let mut forked_msg = msg;
                    forked_msg.id = uuid::Uuid::new_v4().to_string();
                    forked_msg.prompt_turn_id = new_turn_id.clone();
                    messages.insert(forked_msg.id.clone(), forked_msg);
                }
            }
        }

        let mut forked_session = new_session;
        forked_session.forked_from_session_id = Some(source_session_id.to_string());
        forked_session.fork_point_turn_id = Some(fork_point_turn_id.to_string());
        if let Some(new_id) = turn_id_map.get(fork_point_turn_id) {
            forked_session.head_prompt_turn_id = Some(new_id.clone());
        }

        let mut sessions = self.sessions.write().await;
        sessions.insert(forked_session.id.clone(), forked_session);
        Ok(())
    }

    async fn clear(&self) -> Result<(), StoreError> {
        let mut sessions = self.sessions.write().await;
        sessions.clear();
        let mut prompt_turns = self.prompt_turns.write().await;
        prompt_turns.clear();
        let mut messages = self.messages.write().await;
        messages.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{
        run_context_tests, run_fork_tests, run_message_tests, run_prompt_turn_tests,
        run_session_tests,
    };

    #[tokio::test]
    async fn test_session_crud() {
        let store = InMemorySessionStore::new();
        run_session_tests(&store).await;
    }

    #[tokio::test]
    async fn test_prompt_turn_ops() {
        let store = InMemorySessionStore::new();
        run_prompt_turn_tests(&store).await;
    }

    #[tokio::test]
    async fn test_message_ops() {
        let store = InMemorySessionStore::new();
        run_message_tests(&store).await;
    }

    #[tokio::test]
    async fn test_context_assembly() {
        let store = InMemorySessionStore::new();
        run_context_tests(&store).await;
    }

    #[tokio::test]
    async fn test_fork() {
        let store = InMemorySessionStore::new();
        run_fork_tests(&store).await;
    }
}
