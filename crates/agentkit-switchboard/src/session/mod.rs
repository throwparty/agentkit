pub mod sqlite;

use std::fmt;

/// Records which provider a session is pinned to.
///
/// Semantics: last-record-wins. On the first request for a session, the
/// chosen provider is assigned. On subsequent requests the provider may
/// switch (e.g. due to degradation or cost), in which case the record is
/// updated in-place — there is no history. Only the canonical assignment is
/// stored.
///
/// `switch_count` in the database tracks how many times the provider has
/// changed over the session's lifetime.
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
}

#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    pub active_sessions: u64,
    pub total_sessions: u64,
}

#[derive(Debug)]
pub enum SessionError {
    PoolClosed,
    PoolTimeout,
    QueryFailed(String),
    NotFound,
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PoolClosed => write!(f, "database pool closed"),
            Self::PoolTimeout => write!(f, "database pool timeout"),
            Self::QueryFailed(msg) => write!(f, "query failed: {msg}"),
            Self::NotFound => write!(f, "session not found"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<sqlx::Error> for SessionError {
    fn from(err: sqlx::Error) -> Self {
        match err {
            sqlx::Error::PoolClosed => Self::PoolClosed,
            sqlx::Error::PoolTimedOut => Self::PoolTimeout,
            other => Self::QueryFailed(other.to_string()),
        }
    }
}


