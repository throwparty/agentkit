//! The store data model: ids, the session/turn/message records, their kinds and usage, and the store error type.

use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub type SessionId = String;
pub type TurnId = String;
pub type MessageId = String;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("no such session: {0}")]
    NotFound(SessionId),
    #[error("store is misconfigured: {0}")]
    Config(String),
    #[error("session {session} is actively owned by {owner}")]
    LeaseHeld { session: SessionId, owner: String },
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migration failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// One message in assembled context, tagged with its turn's kind (the
/// agent loop needs the boundary information: summaries are untrusted,
/// harness turns are not user-authored).
#[derive(Debug, Clone, PartialEq)]
pub struct AssembledMessage {
    pub message: Message,
    pub turn_kind: TurnKind,
}

/// The session metadata's `model` override, if set — the config-option
/// switch, effective the following turn.
pub fn session_model_free(session: &Session) -> Option<String> {
    metadata_value(session, "model")
}

/// The session metadata's `actor`, if set.
pub fn session_actor_value_free(session: &Session) -> Option<String> {
    metadata_value(session, "actor")
}

fn metadata_value(session: &Session, key: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(&session.metadata)
        .ok()
        .and_then(|metadata| metadata.get(key)?.as_str().map(str::to_owned))
}

pub(super) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Interactive,
    Ephemeral,
}

impl SessionKind {
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            SessionKind::Interactive => "interactive",
            SessionKind::Ephemeral => "ephemeral",
        }
    }

    pub(super) fn from_str(raw: &str) -> Self {
        match raw {
            "ephemeral" => SessionKind::Ephemeral,
            _ => SessionKind::Interactive,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnKind {
    Interaction,
    Seed,
    Compaction,
}

impl TurnKind {
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            TurnKind::Interaction => "interaction",
            TurnKind::Seed => "seed",
            TurnKind::Compaction => "compaction",
        }
    }

    pub(super) fn from_str(raw: &str) -> Self {
        match raw {
            "seed" => TurnKind::Seed,
            "compaction" => TurnKind::Compaction,
            _ => TurnKind::Interaction,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    ToolCall,
    ToolResult,
    System,
}

impl Role {
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::ToolCall => "tool_call",
            Role::ToolResult => "tool_result",
            Role::System => "system",
        }
    }

    pub(super) fn from_str(raw: &str) -> Self {
        match raw {
            "assistant" => Role::Assistant,
            "tool_call" => Role::ToolCall,
            "tool_result" => Role::ToolResult,
            "system" => Role::System,
            _ => Role::User,
        }
    }
}

/// Usage delta incurred by one turn's model requests. Session totals are
/// the sum over the session's OWN turns; ancestor turns' costs belong to
/// their creating sessions, so forks never double-count.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

impl TurnUsage {
    #[cfg(test)]
    pub(super) fn add(self, other: Self) -> Self {
        Self {
            input_tokens: self.input_tokens + other.input_tokens,
            output_tokens: self.output_tokens + other.output_tokens,
            cost_usd: self.cost_usd + other.cost_usd,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: SessionId,
    pub head_turn_id: Option<TurnId>,
    pub kind: SessionKind,
    pub forked_from_session_id: Option<SessionId>,
    pub fork_point_turn_id: Option<TurnId>,
    pub cwd: String,
    pub title: String,
    /// The live ownership lease, if any: the owning connection id and the
    /// absolute expiry (unix seconds).
    pub owner: Option<String>,
    pub lease_expires_at: Option<i64>,
    pub active: bool,
    pub created_at: i64,
    pub updated_at: i64,
    /// JSON blob: session-creation metadata (the selected actor, …).
    pub metadata: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub id: TurnId,
    pub session_id: SessionId,
    pub parent_id: Option<TurnId>,
    pub kind: TurnKind,
    pub first_retained_turn_id: Option<TurnId>,
    pub created_at: i64,
    pub usage: TurnUsage,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub id: MessageId,
    pub turn_id: TurnId,
    pub role: Role,
    /// JSON-serialised ACP content blocks.
    pub content: String,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<MessageId>,
    pub is_error: Option<bool>,
    pub position: i64,
    pub created_at: i64,
}

/// Options for `list_sessions`.
#[derive(Debug, Clone, Default)]
pub struct ListFilter<'a> {
    /// Restrict to sessions with this working directory.
    pub cwd: Option<&'a str>,
    /// Include ephemeral sessions (default: excluded).
    pub include_ephemeral: bool,
    /// Include soft-deleted sessions (default: excluded).
    pub include_deleted: bool,
}

/// A uuid v4 string.
pub(super) fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
