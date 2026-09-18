//! Session storage: the turn DAG behind a `SessionStore` with two
//! interchangeable backends — SQLite (durable, WAL, multi-process) and
//! in-memory (tests). Adopted from adrs/2026-06-13-acp-server-session-storage
//! with the tackle schema extensions: turn kind, first_retained_turn_id,
//! per-turn usage deltas, attribution metadata.
//!
//! Ownership leases (T-008) and context assembly (T-007) build on this.

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub type SessionId = String;
pub type TurnId = String;
pub type MessageId = String;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("store is misconfigured: {0}")]
    Config(String),
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migration failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

fn unix_now() -> i64 {
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
    fn as_str(&self) -> &'static str {
        match self {
            SessionKind::Interactive => "interactive",
            SessionKind::Ephemeral => "ephemeral",
        }
    }

    fn from_str(raw: &str) -> Self {
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
    fn as_str(&self) -> &'static str {
        match self {
            TurnKind::Interaction => "interaction",
            TurnKind::Seed => "seed",
            TurnKind::Compaction => "compaction",
        }
    }

    #[expect(dead_code)] // read path lands with context assembly (T-007) and replay (T-012)
    fn from_str(raw: &str) -> Self {
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
    fn as_str(&self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::ToolCall => "tool_call",
            Role::ToolResult => "tool_result",
            Role::System => "system",
        }
    }

    #[expect(dead_code)] // read path lands with context assembly (T-007) and replay (T-012)
    fn from_str(raw: &str) -> Self {
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
    fn add(self, other: Self) -> Self {
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
    pub active: bool,
    pub created_at: i64,
    pub updated_at: i64,
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
fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// The session store: SQLite or in-memory backend, behind one type.
pub struct SessionStore {
    backend: Backend,
}

enum Backend {
    Sqlite(SqlitePool),
    Memory(Mutex<memory::MemoryData>),
}

impl SessionStore {
    /// Locks the in-memory backend, recovering from poisoning (a panicked
    /// script must not take the store down).
    fn lock_memory(&self) -> MutexGuard<'_, memory::MemoryData> {
        match &self.backend {
            Backend::Memory(data) => match data.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            },
            Backend::Sqlite(_) => unreachable!("lock_memory on sqlite backend"),
        }
    }
}

impl SessionStore {
    /// Opens (creating if absent) the SQLite store at `path`, running all
    /// pending migrations. The database file gets 0600 permissions.
    pub async fn connect_sqlite(path: &Path) -> Result<Self, StoreError> {
        // create_if_missing covers the file only: the leading
        // directories (the data dir) are ours to make.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(StoreError::Io)?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(5));
        let pool = SqlitePoolOptions::new().connect_with(options).await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        restrict_permissions(path)?;

        Ok(Self {
            backend: Backend::Sqlite(pool),
        })
    }

    /// An empty in-memory store: the test backend.
    pub fn in_memory() -> Self {
        Self {
            backend: Backend::Memory(Mutex::new(memory::MemoryData::default())),
        }
    }

    pub async fn create_session(
        &self,
        kind: SessionKind,
        cwd: &str,
        forked_from: Option<&SessionId>,
        fork_point: Option<&TurnId>,
    ) -> Result<Session, StoreError> {
        let id = new_id();
        let now = unix_now();
        let session = Session {
            id: id.clone(),
            head_turn_id: None,
            kind,
            forked_from_session_id: forked_from.cloned(),
            fork_point_turn_id: fork_point.cloned(),
            cwd: cwd.to_owned(),
            title: String::new(),
            active: true,
            created_at: now,
            updated_at: now,
        };
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlx::query(
                    "INSERT INTO sessions (id, kind, forked_from_session_id, fork_point_turn_id, cwd, created_at, updated_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&session.id)
                .bind(kind.as_str())
                .bind(forked_from)
                .bind(fork_point)
                .bind(cwd)
                .bind(now)
                .bind(now)
                .execute(pool)
                .await?;
            }
            Backend::Memory(_) => self.lock_memory().create_session(session.clone()),
        }
        Ok(session)
    }

    pub async fn get_session(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                let row = sqlx::query_as::<_, SqliteSession>("SELECT * FROM sessions WHERE id = ?")
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
                Ok(row.map(SqliteSession::into_domain))
            }
            Backend::Memory(_) => Ok(self.lock_memory().get_session(id)),
        }
    }

    pub async fn list_sessions(&self, filter: &ListFilter<'_>) -> Result<Vec<Session>, StoreError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                // Static query with parameterised filters — no dynamic SQL.
                let rows = sqlx::query_as::<_, SqliteSession>(
                    "SELECT * FROM sessions \
                     WHERE (?1 = 1 OR kind = 'interactive') \
                       AND (?2 = 1 OR active = 1) \
                       AND (?3 IS NULL OR cwd = ?3) \
                     ORDER BY updated_at DESC",
                )
                .bind(filter.include_ephemeral as i64)
                .bind(filter.include_deleted as i64)
                .bind(filter.cwd)
                .fetch_all(pool)
                .await?;
                Ok(rows.into_iter().map(SqliteSession::into_domain).collect())
            }
            Backend::Memory(_) => Ok(self.lock_memory().list_sessions(filter)),
        }
    }

    /// Closes a session: cancels ongoing work and releases the lease. The
    /// session stays active and listable.
    pub async fn close_session(&self, id: &SessionId) -> Result<(), StoreError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlx::query("UPDATE sessions SET owner_connection = NULL, lease_expires_at = NULL, updated_at = ? WHERE id = ?")
                    .bind(unix_now())
                    .bind(id)
                    .execute(pool)
                    .await?;
            }
            Backend::Memory(_) => self.lock_memory().close_session(id),
        }
        Ok(())
    }

    /// Soft-deletes a session: hidden from listings, turns preserved for
    /// forks that reference them.
    pub async fn delete_session(&self, id: &SessionId) -> Result<(), StoreError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlx::query("UPDATE sessions SET active = 0, updated_at = ? WHERE id = ?")
                    .bind(unix_now())
                    .bind(id)
                    .execute(pool)
                    .await?;
            }
            Backend::Memory(_) => self.lock_memory().delete_session(id),
        }
        Ok(())
    }

    /// Appends a turn to a session, updating the session's head.
    pub async fn append_turn(
        &self,
        session_id: &SessionId,
        parent_id: Option<&TurnId>,
        kind: TurnKind,
        first_retained: Option<&TurnId>,
        usage: TurnUsage,
    ) -> Result<Turn, StoreError> {
        let id = new_id();
        let now = unix_now();
        let turn = Turn {
            id: id.clone(),
            session_id: session_id.clone(),
            parent_id: parent_id.cloned(),
            kind,
            first_retained_turn_id: first_retained.cloned(),
            created_at: now,
            usage,
        };
        match &self.backend {
            Backend::Sqlite(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query(
                    "INSERT INTO turns (id, session_id, parent_id, kind, first_retained_turn_id, created_at, input_tokens, output_tokens, cost_usd) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&turn.id)
                .bind(session_id)
                .bind(parent_id)
                .bind(kind.as_str())
                .bind(first_retained)
                .bind(now)
                .bind(usage.input_tokens as i64)
                .bind(usage.output_tokens as i64)
                .bind(usage.cost_usd)
                .execute(&mut *tx)
                .await?;
                sqlx::query("UPDATE sessions SET head_turn_id = ?, updated_at = ? WHERE id = ?")
                    .bind(&turn.id)
                    .bind(now)
                    .bind(session_id)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
            }
            Backend::Memory(_) => self.lock_memory().append_turn(turn.clone()),
        }
        Ok(turn)
    }

    /// Appends a message to a turn at the next position.
    pub async fn append_message(
        &self,
        turn_id: &TurnId,
        role: Role,
        content: &str,
        tool_name: Option<&str>,
        tool_call_id: Option<&MessageId>,
        is_error: Option<bool>,
    ) -> Result<Message, StoreError> {
        let id = new_id();
        let now = unix_now();
        let message = Message {
            id,
            turn_id: turn_id.clone(),
            role,
            content: content.to_owned(),
            tool_name: tool_name.map(str::to_owned),
            tool_call_id: tool_call_id.cloned(),
            is_error,
            position: 0,
            created_at: now,
        };
        match &self.backend {
            Backend::Sqlite(pool) => {
                let next: Option<i64> =
                    sqlx::query_scalar("SELECT MAX(position) + 1 FROM messages WHERE turn_id = ?")
                        .bind(turn_id)
                        .fetch_one(pool)
                        .await?;
                let position = next.unwrap_or(0);
                sqlx::query(
                    "INSERT INTO messages (id, turn_id, role, content, tool_name, tool_call_id, is_error, position, created_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&message.id)
                .bind(turn_id)
                .bind(role.as_str())
                .bind(content)
                .bind(tool_name)
                .bind(tool_call_id)
                .bind(is_error)
                .bind(position)
                .bind(now)
                .execute(pool)
                .await?;
                let mut message = message;
                message.position = position;
                Ok(message)
            }
            Backend::Memory(_) => Ok(self.lock_memory().append_message(message)),
        }
    }

    /// The summed usage over the session's OWN turns (ancestors from other
    /// sessions are excluded — their costs belong to their creators).
    pub async fn session_usage(&self, session_id: &SessionId) -> Result<TurnUsage, StoreError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                let row = sqlx::query_as::<_, (i64, i64, f64)>(
                    "SELECT COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COALESCE(SUM(cost_usd), 0.0) \
                     FROM turns WHERE session_id = ?",
                )
                .bind(session_id)
                .fetch_one(pool)
                .await?;
                Ok(TurnUsage {
                    input_tokens: row.0.max(0) as u64,
                    output_tokens: row.1.max(0) as u64,
                    cost_usd: row.2,
                })
            }
            Backend::Memory(_) => Ok(self.lock_memory().session_usage(session_id)),
        }
    }
}

/// Restricts a file's permissions to 0600 on Unix; no-op elsewhere.
fn restrict_permissions(path: &Path) -> Result<(), StoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }
    let _ = path;
    Ok(())
}

/// Row mapping for the SQLite backend.
#[derive(sqlx::FromRow)]
struct SqliteSession {
    id: String,
    head_turn_id: Option<String>,
    kind: String,
    forked_from_session_id: Option<String>,
    fork_point_turn_id: Option<String>,
    cwd: String,
    title: String,
    active: i64,
    created_at: i64,
    updated_at: i64,
}

impl SqliteSession {
    fn into_domain(self) -> Session {
        Session {
            id: self.id,
            head_turn_id: self.head_turn_id,
            kind: SessionKind::from_str(&self.kind),
            forked_from_session_id: self.forked_from_session_id,
            fork_point_turn_id: self.fork_point_turn_id,
            cwd: self.cwd,
            title: self.title,
            active: self.active != 0,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

mod memory;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sqlite_connects_migrates_and_restricts_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        let store = SessionStore::connect_sqlite(&path).await.unwrap();

        // Migration ran: sessions table is queryable.
        let sessions = store.list_sessions(&ListFilter::default()).await.unwrap();
        assert!(sessions.is_empty());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[tokio::test]
    async fn sqlite_connects_through_missing_leading_directories() {
        let dir = tempfile::tempdir().unwrap();
        // The data dir's full chain may not exist yet — the store
        // creates it rather than failing to open.
        let path = dir.path().join("state/agentkit/tackle/sessions.db");
        assert!(!path.parent().unwrap().exists());
        SessionStore::connect_sqlite(&path).await.unwrap();
        assert!(path.is_file());
    }

    #[tokio::test]
    async fn session_crud_round_trips_on_memory() {
        let store = SessionStore::in_memory();
        crud_round_trip(&store).await.unwrap();
    }

    #[tokio::test]
    async fn session_crud_round_trips_on_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::connect_sqlite(&dir.path().join("sessions.db"))
            .await
            .unwrap();
        crud_round_trip(&store).await.unwrap();
    }

    async fn crud_round_trip(store: &SessionStore) -> Result<(), StoreError> {
        let session = store
            .create_session(SessionKind::Interactive, "/work", None, None)
            .await?;
        assert!(session.head_turn_id.is_none());

        // Update-list requires a later updated_at for ordering; both created
        // in the same second, so ordering assertions only cover membership.
        let listed = store.list_sessions(&ListFilter::default()).await?;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, session.id);

        store.close_session(&session.id).await?;
        let listed = store.list_sessions(&ListFilter::default()).await?;
        assert_eq!(listed.len(), 1, "close keeps the session listable");

        store.delete_session(&session.id).await?;
        let listed = store.list_sessions(&ListFilter::default()).await?;
        assert!(listed.is_empty(), "delete hides the session");

        let deleted = store.get_session(&session.id).await?;
        assert!(deleted.is_some_and(|s| !s.active));
        Ok(())
    }

    #[tokio::test]
    async fn turns_messages_and_usage_round_trip() {
        let store = SessionStore::in_memory();
        let session = store
            .create_session(SessionKind::Interactive, "/work", None, None)
            .await
            .unwrap();

        let turn = store
            .append_turn(
                &session.id,
                None,
                TurnKind::Interaction,
                None,
                TurnUsage {
                    input_tokens: 100,
                    output_tokens: 40,
                    cost_usd: 0.01,
                },
            )
            .await
            .unwrap();
        store
            .append_message(
                &turn.id,
                Role::User,
                "[{\"type\":\"text\"}]",
                None,
                None,
                None,
            )
            .await
            .unwrap();
        store
            .append_message(&turn.id, Role::Assistant, "[]", None, None, None)
            .await
            .unwrap();

        let usage = store.session_usage(&session.id).await.unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 40);
    }

    #[tokio::test]
    async fn ephemeral_sessions_are_filtered_from_listings() {
        let store = SessionStore::in_memory();
        store
            .create_session(SessionKind::Ephemeral, "/work", None, None)
            .await
            .unwrap();

        let listed = store.list_sessions(&ListFilter::default()).await.unwrap();
        assert!(listed.is_empty());

        let listed = store
            .list_sessions(&ListFilter {
                include_ephemeral: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
    }
}
