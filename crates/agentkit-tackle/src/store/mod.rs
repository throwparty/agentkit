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
    #[error("session {session} is actively owned by {owner}")]
    LeaseHeld { session: SessionId, owner: String },
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migration failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

mod graph;
mod memory;

pub use graph::SessionGraph;

/// One message in assembled context, tagged with its turn's kind (the
/// agent loop needs the boundary information: summaries are untrusted,
/// harness turns are not user-authored).
#[derive(Debug, Clone, PartialEq)]
pub struct AssembledMessage {
    pub message: Message,
    pub turn_kind: TurnKind,
}

/// Orders a parent-chain walk (newest first) for assembly: compaction
/// summaries first — they replace the elided prefix — then the retained
/// turns oldest to newest.
pub(crate) fn assembly_order(walk: Vec<(TurnId, TurnKind)>) -> Vec<TurnId> {
    let mut summaries = Vec::new();
    let mut retained = Vec::new();
    for (id, kind) in walk {
        if kind == TurnKind::Compaction {
            summaries.push(id);
        } else {
            retained.push(id);
        }
    }
    summaries.reverse();
    retained.reverse();
    summaries.extend(retained);
    summaries
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
        metadata: &str,
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
            owner: None,
            lease_expires_at: None,
            active: true,
            created_at: now,
            updated_at: now,
            metadata: metadata.to_owned(),
        };
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlx::query(
                    "INSERT INTO sessions (id, kind, forked_from_session_id, fork_point_turn_id, cwd, metadata, created_at, updated_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&session.id)
                .bind(kind.as_str())
                .bind(forked_from)
                .bind(fork_point)
                .bind(cwd)
                .bind(metadata)
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

    /// Attempts to acquire the session's ownership lease for `owner` for
    /// `ttl_secs`. Succeeds if the lease is free or expired. A session
    /// actively owned by another connection must not accept turns.
    pub async fn acquire_lease(
        &self,
        session_id: &SessionId,
        owner: &str,
        ttl_secs: i64,
    ) -> Result<bool, StoreError> {
        let now = unix_now();
        match &self.backend {
            Backend::Sqlite(pool) => {
                let result = sqlx::query(
                    "UPDATE sessions SET owner_connection = ?, lease_expires_at = ? \
                     WHERE id = ? AND (owner_connection IS NULL OR lease_expires_at < ?)",
                )
                .bind(owner)
                .bind(now + ttl_secs)
                .bind(session_id)
                .bind(now)
                .execute(pool)
                .await?;
                Ok(result.rows_affected() == 1)
            }
            Backend::Memory(_) => {
                Ok(self
                    .lock_memory()
                    .acquire_lease(session_id, owner, now + ttl_secs, now))
            }
        }
    }

    /// Refreshes the lease; false if another connection holds it.
    pub async fn heartbeat_lease(
        &self,
        session_id: &SessionId,
        owner: &str,
        ttl_secs: i64,
    ) -> Result<bool, StoreError> {
        let now = unix_now();
        match &self.backend {
            Backend::Sqlite(pool) => {
                let result = sqlx::query(
                    "UPDATE sessions SET lease_expires_at = ? \
                     WHERE id = ? AND owner_connection = ?",
                )
                .bind(now + ttl_secs)
                .bind(session_id)
                .bind(owner)
                .execute(pool)
                .await?;
                Ok(result.rows_affected() == 1)
            }
            Backend::Memory(_) => {
                Ok(self
                    .lock_memory()
                    .heartbeat_lease(session_id, owner, now + ttl_secs))
            }
        }
    }

    /// The current lease holder, if the lease is live.
    pub async fn lease_holder(&self, session_id: &SessionId) -> Result<Option<String>, StoreError> {
        let now = unix_now();
        match &self.backend {
            Backend::Sqlite(pool) => {
                let owner: Option<String> = sqlx::query_scalar(
                    "SELECT owner_connection FROM sessions \
                     WHERE id = ? AND lease_expires_at >= ?",
                )
                .bind(session_id)
                .bind(now)
                .fetch_optional(pool)
                .await?
                .flatten();
                Ok(owner)
            }
            Backend::Memory(_) => Ok(self.lock_memory().lease_holder(session_id, now)),
        }
    }

    /// Soft-deletes a session: hidden from listings, turns preserved for
    /// forks that reference them. Refused while the session is actively
    /// leased; close first.
    pub async fn delete_session(&self, id: &SessionId) -> Result<(), StoreError> {
        let now = unix_now();
        match &self.backend {
            Backend::Sqlite(pool) => {
                let result = sqlx::query(
                    "UPDATE sessions SET active = 0, updated_at = ? \
                     WHERE id = ? AND (owner_connection IS NULL OR lease_expires_at < ?)",
                )
                .bind(now)
                .bind(id)
                .bind(now)
                .execute(pool)
                .await?;
                if result.rows_affected() == 0 {
                    if let Some(owner) = self.lease_holder(id).await? {
                        return Err(StoreError::LeaseHeld {
                            session: id.clone(),
                            owner,
                        });
                    }
                    // No rows and no live lease: unknown or already deleted —
                    // idempotent success.
                }
            }
            Backend::Memory(_) => self.lock_memory().delete_session(id, now)?,
        }
        Ok(())
    }

    /// Releases the lease if still held by `owner`.
    pub async fn release_lease(
        &self,
        session_id: &SessionId,
        owner: &str,
    ) -> Result<(), StoreError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlx::query(
                    "UPDATE sessions SET owner_connection = NULL, lease_expires_at = NULL \
                     WHERE id = ? AND owner_connection = ?",
                )
                .bind(session_id)
                .bind(owner)
                .execute(pool)
                .await?;
            }
            Backend::Memory(_) => self.lock_memory().release_lease(session_id, owner),
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

    /// Appends a message to a turn at the next position. `id` supplies a
    /// pre-generated identity (streaming chunks and the stored message
    /// share one); `None` generates one.
    #[allow(clippy::too_many_arguments)] // the tool-message fields are all optional and positional
    pub async fn append_message(
        &self,
        turn_id: &TurnId,
        role: Role,
        content: &str,
        tool_name: Option<&str>,
        tool_call_id: Option<&MessageId>,
        is_error: Option<bool>,
        id: Option<&str>,
    ) -> Result<Message, StoreError> {
        let id = id.map(str::to_owned).unwrap_or_else(new_id);
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

    /// Records the usage delta a turn's model requests incurred.
    pub async fn set_turn_usage(
        &self,
        turn_id: &TurnId,
        usage: TurnUsage,
    ) -> Result<(), StoreError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlx::query(
                    "UPDATE turns SET input_tokens = ?, output_tokens = ?, cost_usd = ? WHERE id = ?",
                )
                .bind(usage.input_tokens as i64)
                .bind(usage.output_tokens as i64)
                .bind(usage.cost_usd)
                .bind(turn_id)
                .execute(pool)
                .await?;
            }
            Backend::Memory(_) => self.lock_memory().set_turn_usage(turn_id, usage),
        }
        Ok(())
    }

    /// Assembles the conversation context for a session: the parent-chain
    /// walk from the head with compaction truncation, summaries first,
    /// then retained turns oldest to newest, messages expanded in
    /// position order.
    pub async fn assemble_context(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<AssembledMessage>, StoreError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                let rows = sqlx::query_as::<_, SqliteAssembledRow>(
                    "WITH RECURSIVE walk AS ( \
                         SELECT t.*, 0 AS depth, CAST(NULL AS TEXT) AS stop_at \
                         FROM turns t \
                         JOIN sessions s ON s.head_turn_id = t.id \
                         WHERE s.id = ?1 \
                         UNION ALL \
                         SELECT p.*, walk.depth + 1, \
                                CASE WHEN walk.kind = 'compaction' \
                                     THEN walk.first_retained_turn_id \
                                     ELSE walk.stop_at \
                                END \
                         FROM walk \
                         JOIN turns p ON p.id = walk.parent_id \
                         WHERE NOT (walk.kind = 'compaction' AND walk.first_retained_turn_id IS NULL) \
                           AND (walk.stop_at IS NULL OR walk.id <> walk.stop_at) \
                     ) \
                     SELECT m.id, m.turn_id, m.role, m.content, m.tool_name, m.tool_call_id, m.is_error, m.position, m.created_at, w.kind \
                     FROM walk w \
                     JOIN messages m ON m.turn_id = w.id \
                     ORDER BY CASE WHEN w.kind = 'compaction' THEN 0 ELSE 1 END, w.depth DESC, m.position ASC",
                )
                .bind(session_id)
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(SqliteAssembledRow::into_domain)
                    .collect())
            }
            Backend::Memory(_) => Ok(self.lock_memory().assemble_context(session_id)),
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
    owner_connection: Option<String>,
    lease_expires_at: Option<i64>,
    active: i64,
    created_at: i64,
    updated_at: i64,
    metadata: String,
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
            owner: self.owner_connection,
            lease_expires_at: self.lease_expires_at,
            active: self.active != 0,
            created_at: self.created_at,
            updated_at: self.updated_at,
            metadata: self.metadata,
        }
    }
}

/// Row mapping for the assembly CTE.
#[derive(sqlx::FromRow)]
struct SqliteAssembledRow {
    id: String,
    turn_id: String,
    role: String,
    content: String,
    tool_name: Option<String>,
    tool_call_id: Option<String>,
    is_error: Option<i64>,
    position: i64,
    created_at: i64,
    kind: String,
}

impl SqliteAssembledRow {
    fn into_domain(self) -> AssembledMessage {
        AssembledMessage {
            message: Message {
                id: self.id,
                turn_id: self.turn_id,
                role: Role::from_str(&self.role),
                content: self.content,
                tool_name: self.tool_name,
                tool_call_id: self.tool_call_id,
                is_error: self.is_error.map(|v| v != 0),
                position: self.position,
                created_at: self.created_at,
            },
            turn_kind: TurnKind::from_str(&self.kind),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

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
            .create_session(SessionKind::Interactive, "/work", None, None, "{}")
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
            .create_session(SessionKind::Interactive, "/work", None, None, "{}")
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
                None,
            )
            .await
            .unwrap();
        store
            .append_message(&turn.id, Role::Assistant, "[]", None, None, None, None)
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
            .create_session(SessionKind::Ephemeral, "/work", None, None, "{}")
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

    // --- Context assembly (T-007) ---

    /// Appends one interaction turn with a single user message whose
    /// content identifies the turn.
    async fn append_named_turn(
        store: &SessionStore,
        session_id: &SessionId,
        parent: Option<&TurnId>,
        kind: TurnKind,
        first_retained: Option<&TurnId>,
        content: &str,
    ) -> Turn {
        let turn = store
            .append_turn(
                session_id,
                parent,
                kind,
                first_retained,
                TurnUsage::default(),
            )
            .await
            .unwrap();
        store
            .append_message(
                &turn.id,
                Role::User,
                &format!("[{{\"type\":\"text\",\"text\":\"{content}\"}}]"),
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        turn
    }

    fn assembled_texts(messages: &[AssembledMessage]) -> Vec<String> {
        messages
            .iter()
            .map(|assembled| {
                let blocks: serde_json::Value =
                    serde_json::from_str(&assembled.message.content).unwrap();
                blocks[0]["text"].as_str().expect("text block").to_owned()
            })
            .collect()
    }

    /// An in-memory SQLite store: the fast backend for property tests.
    /// The pool is pinned to one connection — each pooled connection would
    /// otherwise get its own empty `:memory:` database.
    async fn sqlite_memory_store() -> SessionStore {
        let options = SqliteConnectOptions::new()
            .in_memory(true)
            .busy_timeout(std::time::Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        SessionStore {
            backend: Backend::Sqlite(pool),
        }
    }

    #[tokio::test]
    async fn linear_chain_assembles_oldest_first() {
        for store in [SessionStore::in_memory(), sqlite_memory_store().await] {
            let session = store
                .create_session(SessionKind::Interactive, "/work", None, None, "{}")
                .await
                .unwrap();
            let mut parent = None;
            for name in ["t1", "t2", "t3"] {
                let turn = append_named_turn(
                    &store,
                    &session.id,
                    parent.as_ref(),
                    TurnKind::Interaction,
                    None,
                    name,
                )
                .await;
                parent = Some(turn.id);
            }
            let assembled = store.assemble_context(&session.id).await.unwrap();
            assert_eq!(assembled_texts(&assembled), ["t1", "t2", "t3"]);
        }
    }

    #[tokio::test]
    async fn full_compaction_elides_everything_below_the_summary() {
        for store in [SessionStore::in_memory(), sqlite_memory_store().await] {
            let session = store
                .create_session(SessionKind::Interactive, "/work", None, None, "{}")
                .await
                .unwrap();
            let t1 =
                append_named_turn(&store, &session.id, None, TurnKind::Interaction, None, "t1")
                    .await;
            let t2 = append_named_turn(
                &store,
                &session.id,
                Some(&t1.id),
                TurnKind::Interaction,
                None,
                "t2",
            )
            .await;
            append_named_turn(
                &store,
                &session.id,
                Some(&t2.id),
                TurnKind::Compaction,
                None,
                "summary",
            )
            .await;

            let assembled = store.assemble_context(&session.id).await.unwrap();
            assert_eq!(
                assembled_texts(&assembled),
                ["summary"],
                "full compaction keeps only the summary"
            );
        }
    }

    #[tokio::test]
    async fn keep_recent_compaction_retains_range_and_summary_sorts_first() {
        for store in [SessionStore::in_memory(), sqlite_memory_store().await] {
            let session = store
                .create_session(SessionKind::Interactive, "/work", None, None, "{}")
                .await
                .unwrap();
            let t1 =
                append_named_turn(&store, &session.id, None, TurnKind::Interaction, None, "t1")
                    .await;
            let t2 = append_named_turn(
                &store,
                &session.id,
                Some(&t1.id),
                TurnKind::Interaction,
                None,
                "t2",
            )
            .await;
            let t3 = append_named_turn(
                &store,
                &session.id,
                Some(&t2.id),
                TurnKind::Interaction,
                None,
                "t3",
            )
            .await;
            let t4 = append_named_turn(
                &store,
                &session.id,
                Some(&t3.id),
                TurnKind::Interaction,
                None,
                "t4",
            )
            .await;
            // Compaction at t5 keeps t3 onward verbatim; t1..t2 are elided.
            append_named_turn(
                &store,
                &session.id,
                Some(&t4.id),
                TurnKind::Compaction,
                Some(&t3.id),
                "summary",
            )
            .await;

            let assembled = store.assemble_context(&session.id).await.unwrap();
            assert_eq!(
                assembled_texts(&assembled),
                ["summary", "t3", "t4"],
                "summary first, then retained turns oldest to newest"
            );
        }
    }

    #[tokio::test]
    async fn turns_after_compaction_come_after_the_retained_range() {
        for store in [SessionStore::in_memory(), sqlite_memory_store().await] {
            let session = store
                .create_session(SessionKind::Interactive, "/work", None, None, "{}")
                .await
                .unwrap();
            let t1 =
                append_named_turn(&store, &session.id, None, TurnKind::Interaction, None, "t1")
                    .await;
            let c = append_named_turn(
                &store,
                &session.id,
                Some(&t1.id),
                TurnKind::Compaction,
                None,
                "summary",
            )
            .await;
            append_named_turn(
                &store,
                &session.id,
                Some(&c.id),
                TurnKind::Interaction,
                None,
                "t2",
            )
            .await;

            let assembled = store.assemble_context(&session.id).await.unwrap();
            assert_eq!(assembled_texts(&assembled), ["summary", "t2"]);
        }
    }

    /// Seeded xorshift: deterministic property-test generation without a
    /// random-number dependency.
    struct Rng(u64);

    impl Rng {
        fn new(seed: u64) -> Self {
            Self(seed | 1)
        }

        fn below(&mut self, n: u64) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x % n
        }
    }

    #[tokio::test]
    async fn property_sql_matches_memory_and_daggy() {
        for seed in 0..48u64 {
            let mut rng = Rng::new(seed);
            let memory_store = SessionStore::in_memory();
            let sqlite_store = sqlite_memory_store().await;

            let session = memory_store
                .create_session(SessionKind::Interactive, "/work", None, None, "{}")
                .await
                .unwrap();
            let sqlite_session = sqlite_store
                .create_session(SessionKind::Interactive, "/work", None, None, "{}")
                .await
                .unwrap();

            // All turns ever created (the SessionGraph oracle spans the DAG).
            // Backends generate independent uuids; correspondence between
            // memory and sqlite turns is positional, so parent/retained
            // links are chosen as INDICES and mapped per backend.
            let mut all_turns: Vec<Turn> = Vec::new();
            let mut all_turns_sqlite: Vec<Turn> = Vec::new();
            let mut content_by_turn: BTreeMap<TurnId, String> = BTreeMap::new();
            let mut head_idx: Option<usize> = None;
            let mut newest_compaction_idx: Option<usize> = None;

            let count = 8 + rng.below(8);
            for i in 0..count {
                // Fork: occasionally continue from an older turn instead of
                // the head.
                let parent_idx: Option<usize> = if head_idx.is_some() && rng.below(4) == 0 {
                    Some(rng.below(all_turns.len() as u64) as usize)
                } else {
                    head_idx
                };

                let is_compaction = rng.below(5) == 0;
                let (kind, retained_idx) = if is_compaction {
                    // Clamp rule: first_retained must be newer than the
                    // newest existing compaction (or a full compaction).
                    let retained = match newest_compaction_idx {
                        None => {
                            if all_turns.is_empty() {
                                None
                            } else {
                                Some(rng.below(all_turns.len() as u64) as usize)
                            }
                        }
                        Some(newest) => {
                            let newer_span = all_turns.len() - newest - 1;
                            if newer_span > 0 {
                                Some(newest + 1 + rng.below(newer_span as u64) as usize)
                            } else {
                                None
                            }
                        }
                    };
                    (TurnKind::Compaction, retained)
                } else {
                    (TurnKind::Interaction, None)
                };

                let content = format!("s{seed}-t{i}");
                let memory_parent = parent_idx.map(|idx| all_turns[idx].id.clone());
                let sqlite_parent = parent_idx.map(|idx| all_turns_sqlite[idx].id.clone());
                let memory_retained = retained_idx.map(|idx| all_turns[idx].id.clone());
                let sqlite_retained = retained_idx.map(|idx| all_turns_sqlite[idx].id.clone());

                let memory_turn = append_named_turn(
                    &memory_store,
                    &session.id,
                    memory_parent.as_ref(),
                    kind,
                    memory_retained.as_ref(),
                    &content,
                )
                .await;
                // Each backend generates its own uuid; the correspondence
                // is positional, verified via the shared content below.
                let sqlite_turn = append_named_turn(
                    &sqlite_store,
                    &sqlite_session.id,
                    sqlite_parent.as_ref(),
                    kind,
                    sqlite_retained.as_ref(),
                    &content,
                )
                .await;

                content_by_turn.insert(memory_turn.id.clone(), content);
                if kind == TurnKind::Compaction {
                    newest_compaction_idx = Some(all_turns.len());
                }
                head_idx = Some(all_turns.len());
                all_turns.push(memory_turn);
                all_turns_sqlite.push(sqlite_turn);
            }

            // Oracle: daggy graph over all turns, walked from the head.
            let graph = SessionGraph::build(all_turns.iter().cloned());
            let head_id = all_turns[head_idx.unwrap()].id.clone();
            let expected_ids = graph.assembly_order(&head_id);
            let expected_contents: Vec<String> = expected_ids
                .iter()
                .map(|id| content_by_turn[id].clone())
                .collect();

            let memory_assembled = memory_store.assemble_context(&session.id).await.unwrap();
            let sqlite_assembled = sqlite_store
                .assemble_context(&sqlite_session.id)
                .await
                .unwrap();

            assert_eq!(
                assembled_texts(&memory_assembled),
                expected_contents,
                "seed {seed}: memory assembly must match the daggy oracle"
            );
            assert_eq!(
                assembled_texts(&sqlite_assembled),
                expected_contents,
                "seed {seed}: SQL assembly must match the daggy oracle"
            );
        }
    }

    // --- Ownership leases (T-008) ---

    async fn lease_round_trip(store: &SessionStore) -> Result<(), StoreError> {
        let session = store
            .create_session(SessionKind::Interactive, "/work", None, None, "{}")
            .await?;

        // Contention: the first owner wins; the second is refused.
        assert!(store.acquire_lease(&session.id, "conn-a", 60).await?);
        assert!(!store.acquire_lease(&session.id, "conn-b", 60).await?);
        assert_eq!(
            store.lease_holder(&session.id).await?,
            Some("conn-a".into())
        );

        // Heartbeat by the owner refreshes; by another is refused.
        assert!(store.heartbeat_lease(&session.id, "conn-a", 60).await?);
        assert!(!store.heartbeat_lease(&session.id, "conn-b", 60).await?);

        // Soft delete is refused while actively leased.
        let err = store.delete_session(&session.id).await.unwrap_err();
        assert!(
            matches!(&err, StoreError::LeaseHeld { owner, .. } if owner == "conn-a"),
            "{err}"
        );

        // Close releases the lease; delete then succeeds.
        store.close_session(&session.id).await?;
        assert_eq!(store.lease_holder(&session.id).await?, None);
        store.delete_session(&session.id).await?;
        Ok(())
    }

    #[tokio::test]
    async fn lease_round_trips_on_memory() {
        lease_round_trip(&SessionStore::in_memory()).await.unwrap();
    }

    #[tokio::test]
    async fn lease_round_trips_on_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::connect_sqlite(&dir.path().join("sessions.db"))
            .await
            .unwrap();
        lease_round_trip(&store).await.unwrap();
    }

    #[tokio::test]
    async fn expired_leases_are_stealable() -> Result<(), StoreError> {
        for store in [SessionStore::in_memory(), sqlite_memory_store().await] {
            let session = store
                .create_session(SessionKind::Interactive, "/work", None, None, "{}")
                .await?;
            // A lease already expired when acquired: a negative ttl puts
            // the expiry in the past.
            assert!(store.acquire_lease(&session.id, "stale", -1).await?);
            assert!(store.acquire_lease(&session.id, "fresh", 60).await?);
            assert_eq!(store.lease_holder(&session.id).await?, Some("fresh".into()));
        }
        Ok(())
    }
}
