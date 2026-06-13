use async_trait::async_trait;
use thiserror::Error;

use crate::types::{Message, PromptTurn, Session};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("{entity} not found: {id}")]
    NotFound { entity: &'static str, id: String },
    #[error("{entity} already exists: {id}")]
    AlreadyExists { entity: &'static str, id: String },
    #[error("Database error: {0}")]
    Database(String),
}

#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Session CRUD
    async fn create_session(&self, session: Session) -> Result<(), StoreError>;
    async fn get_session(&self, id: &str) -> Result<Session, StoreError>;
    async fn list_sessions(&self) -> Result<Vec<Session>, StoreError>;
    async fn close_session(&self, id: &str) -> Result<(), StoreError>;
    async fn set_session_mode(&self, id: &str, mode: String) -> Result<(), StoreError>;
    async fn set_session_head(
        &self,
        id: &str,
        head_prompt_turn_id: &str,
    ) -> Result<(), StoreError>;

    /// Prompt Turn CRUD
    async fn append_prompt_turn(&self, turn: PromptTurn) -> Result<(), StoreError>;
    async fn get_prompt_turn_children(&self, id: &str) -> Result<Vec<PromptTurn>, StoreError>;
    async fn get_session_prompt_turns(
        &self,
        session_id: &str,
    ) -> Result<Vec<PromptTurn>, StoreError>;

    /// Message CRUD
    async fn append_message(&self, message: Message) -> Result<(), StoreError>;
    async fn get_messages_for_turn(&self, turn_id: &str) -> Result<Vec<Message>, StoreError>;

    /// Context assembly
    async fn get_context(
        &self,
        session_id: &str,
        max_turns: Option<usize>,
    ) -> Result<Vec<Message>, StoreError>;

    /// Fork
    async fn fork_session(
        &self,
        new_session: Session,
        source_session_id: &str,
        fork_point_turn_id: &str,
    ) -> Result<(), StoreError>;

    /// Test support
    async fn clear(&self) -> Result<(), StoreError>;
}
