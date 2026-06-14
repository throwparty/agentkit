pub mod memory;
pub mod sqlite;

use std::fmt;

#[derive(Debug, Clone)]
pub struct SessionAffinity {
    pub session_id: String,
    pub provider_identity: String,
    pub model_name: String,
    pub api_surface: String,
}

#[derive(Debug, Clone)]
pub struct RoutingEvent {
    pub session_id: Option<String>,
    pub request_id: String,
    pub model_name: String,
    pub provider_identity: String,
    pub billing_model: String,
    pub decision_reason: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub response_status: Option<i64>,
    pub latency_ms: Option<i64>,
    pub degraded_providers: Option<String>,
}

#[derive(Debug)]
pub enum SessionError {
    Database(String),
    NotFound,
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(msg) => write!(f, "database error: {msg}"),
            Self::NotFound => write!(f, "session not found"),
        }
    }
}

impl std::error::Error for SessionError {}

#[async_trait::async_trait]
pub trait SessionManager: Send + Sync {
    async fn lookup(&self, session_id: &str) -> Result<Option<SessionAffinity>, SessionError>;
    async fn assign(
        &self,
        session_id: &str,
        provider: &str,
        model: &str,
        surface: &str,
    ) -> Result<(), SessionError>;
    async fn update_tokens(
        &self,
        session_id: &str,
        input: u64,
        output: u64,
    ) -> Result<(), SessionError>;
    async fn increment_switch(
        &self,
        session_id: &str,
        new_provider: &str,
    ) -> Result<(), SessionError>;
    async fn insert_routing_event(&self, event: RoutingEvent) -> Result<(), SessionError>;
}
