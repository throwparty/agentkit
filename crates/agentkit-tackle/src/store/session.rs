//! The `SessionStore` facade: the SQLite backend and the test in-memory backend behind one type, and the operations over them.

#[cfg(test)]
use super::memory;
use super::model::*;
use super::sqlite::{restrict_permissions, SqliteAssembledRow, SqliteSession};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use std::path::Path;
#[cfg(test)]
use std::sync::{Mutex, MutexGuard};

/// The session store: SQLite or in-memory backend, behind one type.
pub struct SessionStore {
    pub(super) backend: Backend,
}

pub(super) enum Backend {
    Sqlite(SqlitePool),
    #[cfg(test)]
    Memory(Mutex<memory::MemoryData>),
}

impl SessionStore {
    /// Locks the in-memory backend, recovering from poisoning (a panicked
    /// script must not take the store down).
    #[cfg(test)]
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
    #[cfg(test)]
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
            #[cfg(test)]
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
            #[cfg(test)]
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
            #[cfg(test)]
            Backend::Memory(_) => Ok(self.lock_memory().list_sessions(filter)),
        }
    }

    /// Closes a session: releases its lease. The session stays active and
    /// listable, and its turns are untouched.
    pub async fn close_session(&self, id: &SessionId) -> Result<(), StoreError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlx::query("UPDATE sessions SET owner_connection = NULL, lease_expires_at = NULL, updated_at = ? WHERE id = ?")
                    .bind(unix_now())
                    .bind(id)
                    .execute(pool)
                    .await?;
            }
            #[cfg(test)]
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
            #[cfg(test)]
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
            #[cfg(test)]
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
            #[cfg(test)]
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
            #[cfg(test)]
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
            #[cfg(test)]
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
            #[cfg(test)]
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
            #[cfg(test)]
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
            #[cfg(test)]
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
            #[cfg(test)]
            Backend::Memory(_) => self.lock_memory().set_turn_usage(turn_id, usage),
        }
        Ok(())
    }

    /// Sets the session's model override (the config-option switch).
    pub async fn set_model(&self, session_id: &SessionId, model: &str) -> Result<(), StoreError> {
        self.update_metadata(session_id, |metadata| {
            metadata["model"] = serde_json::json!(model)
        })
        .await
    }

    /// Sets the session's actor override (the config-option switch).
    pub async fn set_actor(&self, session_id: &SessionId, actor: &str) -> Result<(), StoreError> {
        self.update_metadata(session_id, |metadata| {
            metadata["actor"] = serde_json::json!(actor)
        })
        .await
    }

    async fn update_metadata(
        &self,
        session_id: &SessionId,
        mutate: impl FnOnce(&mut serde_json::Value),
    ) -> Result<(), StoreError> {
        let session = self
            .get_session(session_id)
            .await?
            .ok_or_else(|| StoreError::NotFound(session_id.clone()))?;
        let mut metadata: serde_json::Value =
            serde_json::from_str(&session.metadata).unwrap_or(serde_json::json!({}));
        mutate(&mut metadata);
        let now = unix_now();
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlx::query("UPDATE sessions SET metadata = ?, updated_at = ? WHERE id = ?")
                    .bind(metadata.to_string())
                    .bind(now)
                    .bind(session_id)
                    .execute(pool)
                    .await?;
            }
            #[cfg(test)]
            Backend::Memory(_) => self
                .lock_memory()
                .set_metadata(session_id, &metadata.to_string()),
        }
        Ok(())
    }

    /// Sets the session title (the `session_info_update` backing).
    pub async fn set_title(&self, session_id: &SessionId, title: &str) -> Result<(), StoreError> {
        let now = unix_now();
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlx::query("UPDATE sessions SET title = ?, updated_at = ? WHERE id = ?")
                    .bind(title)
                    .bind(now)
                    .bind(session_id)
                    .execute(pool)
                    .await?;
            }
            #[cfg(test)]
            Backend::Memory(_) => self.lock_memory().set_title(session_id, title),
        }
        Ok(())
    }

    /// The session's parent-chain turn identities and kinds, newest
    /// first — compaction clamping reads the kinds.
    pub async fn session_walk(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<(TurnId, TurnKind)>, StoreError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                let rows: Vec<(String, String)> = sqlx::query_as(
                    "WITH RECURSIVE walk AS ( \
                         SELECT t.*, 0 AS depth \
                         FROM turns t \
                         JOIN sessions s ON s.head_turn_id = t.id \
                         WHERE s.id = ?1 \
                         UNION ALL \
                         SELECT p.*, walk.depth + 1 \
                         FROM walk \
                         JOIN turns p ON p.id = walk.parent_id \
                     ) \
                     SELECT id, kind FROM walk ORDER BY depth ASC",
                )
                .bind(session_id)
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|(id, kind)| (id, TurnKind::from_str(&kind)))
                    .collect())
            }
            #[cfg(test)]
            Backend::Memory(_) => Ok(self.lock_memory().session_walk(session_id)),
        }
    }

    /// One turn by id.
    pub async fn get_turn(&self, turn_id: &TurnId) -> Result<Option<Turn>, StoreError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                let row = sqlx::query_as::<
                    _,
                    (
                        String,
                        String,
                        String,
                        Option<String>,
                        Option<String>,
                        i64,
                        i64,
                        i64,
                        f64,
                    ),
                >(
                    "SELECT id, session_id, kind, parent_id, first_retained_turn_id, created_at, input_tokens, output_tokens, cost_usd FROM turns WHERE id = ?",
                )
                .bind(turn_id)
                .fetch_optional(pool)
                .await?;
                Ok(row.map(
                    |(
                        id,
                        session_id,
                        kind,
                        parent_id,
                        first_retained,
                        created_at,
                        input,
                        output,
                        cost,
                    )| Turn {
                        id,
                        session_id,
                        kind: TurnKind::from_str(&kind),
                        parent_id,
                        first_retained_turn_id: first_retained,
                        created_at,
                        usage: TurnUsage {
                            input_tokens: input.max(0) as u64,
                            output_tokens: output.max(0) as u64,
                            cost_usd: cost,
                        },
                    },
                ))
            }
            #[cfg(test)]
            Backend::Memory(_) => Ok(self.lock_memory().get_turn(turn_id)),
        }
    }

    /// Deletes one turn, restoring full context for compaction turns
    /// (reversibility by deletion). Messages go with it; the head moves
    /// to the deleted turn's parent.
    pub async fn delete_turn(&self, turn_id: &TurnId) -> Result<(), StoreError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                let mut tx = pool.begin().await?;
                let (session_id, parent): (String, Option<String>) =
                    sqlx::query_as("SELECT session_id, parent_id FROM turns WHERE id = ?")
                        .bind(turn_id)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(|err| match err {
                            sqlx::Error::RowNotFound => StoreError::NotFound(turn_id.clone()),
                            other => StoreError::Sqlx(other),
                        })?;
                // Move the head first: sessions.head_turn_id references the
                // turn and foreign keys are immediate.
                sqlx::query(
                    "UPDATE sessions SET head_turn_id = ?, updated_at = ? WHERE id = ? AND head_turn_id = ?",
                )
                .bind(parent)
                .bind(unix_now())
                .bind(session_id)
                .bind(turn_id)
                .execute(&mut *tx)
                .await?;
                sqlx::query("DELETE FROM messages WHERE turn_id = ?")
                    .bind(turn_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("DELETE FROM turns WHERE id = ?")
                    .bind(turn_id)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
            }
            #[cfg(test)]
            Backend::Memory(_) => self.lock_memory().delete_turn(turn_id),
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
            #[cfg(test)]
            Backend::Memory(_) => Ok(self.lock_memory().assemble_context(session_id)),
        }
    }
}
