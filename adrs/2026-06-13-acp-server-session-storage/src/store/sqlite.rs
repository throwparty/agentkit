use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use sqlx::migrate::Migrator;
use sqlx::pool::PoolOptions;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{Sqlite, SqlitePool};

use crate::store::{SessionStore, StoreError};
use crate::types::{Message, PromptTurn, Session};

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn map_sqlx_error(entity: &'static str, id: &str, err: sqlx::Error) -> StoreError {
    match &err {
        sqlx::Error::RowNotFound => StoreError::NotFound {
            entity,
            id: id.to_string(),
        },
        sqlx::Error::Database(dbe) => {
            let is_unique_violation = dbe
                .code()
                .as_deref()
                .is_some_and(|c| c == "1555" || c == "2067");
            if is_unique_violation {
                StoreError::AlreadyExists {
                    entity,
                    id: id.to_string(),
                }
            } else {
                StoreError::Database(err.to_string())
            }
        }
        _ => StoreError::Database(err.to_string()),
    }
}

pub struct SqliteSessionStore {
    pool: SqlitePool,
}

impl SqliteSessionStore {
    pub async fn connect(path: &str) -> Result<Self, StoreError> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_millis(5000));

        let pool = PoolOptions::<Sqlite>::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

        let migrator = Migrator::new(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/migrations"
        )))
        .await
        .map_err(|e| StoreError::Database(e.to_string()))?;
        migrator
            .run(&pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

        Ok(Self { pool })
    }
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn create_session(&self, session: Session) -> Result<(), StoreError> {
        sqlx::query(
            r#"
            INSERT INTO sessions (id, head_prompt_turn_id, forked_from_session_id,
                                  fork_point_turn_id, cwd, title, mode,
                                  created_at, updated_at, active, transport)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&session.id)
        .bind(&session.head_prompt_turn_id)
        .bind(&session.forked_from_session_id)
        .bind(&session.fork_point_turn_id)
        .bind(&session.cwd)
        .bind(&session.title)
        .bind(&session.mode)
        .bind(session.created_at as i64)
        .bind(session.updated_at as i64)
        .bind(session.active as i32)
        .bind(&session.transport)
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(|e| map_sqlx_error("session", &session.id, e))
    }

    async fn get_session(&self, id: &str) -> Result<Session, StoreError> {
        sqlx::query_as::<_, SessionRow>(
            r#"
            SELECT id, head_prompt_turn_id, forked_from_session_id,
                   fork_point_turn_id, cwd, title, mode,
                   created_at, updated_at, active, transport
            FROM sessions WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map(Into::into)
        .map_err(|e| map_sqlx_error("session", id, e))
    }

    async fn list_sessions(&self) -> Result<Vec<Session>, StoreError> {
        let rows = sqlx::query_as::<_, SessionRow>(
            r#"
            SELECT id, head_prompt_turn_id, forked_from_session_id,
                   fork_point_turn_id, cwd, title, mode,
                   created_at, updated_at, active, transport
            FROM sessions ORDER BY updated_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Database(e.to_string()))?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn close_session(&self, id: &str) -> Result<(), StoreError> {
        let ts = now() as i64;
        let rows = sqlx::query(
            r#"
            UPDATE sessions SET active = 0, updated_at = ? WHERE id = ?
            "#,
        )
        .bind(ts)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| map_sqlx_error("session", id, e))?
        .rows_affected();

        if rows == 0 {
            return Err(StoreError::NotFound {
                entity: "session",
                id: id.to_string(),
            });
        }
        Ok(())
    }

    async fn set_session_mode(&self, id: &str, mode: String) -> Result<(), StoreError> {
        let ts = now() as i64;
        let rows = sqlx::query(
            r#"
            UPDATE sessions SET mode = ?, updated_at = ? WHERE id = ?
            "#,
        )
        .bind(&mode)
        .bind(ts)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| map_sqlx_error("session", id, e))?
        .rows_affected();

        if rows == 0 {
            return Err(StoreError::NotFound {
                entity: "session",
                id: id.to_string(),
            });
        }
        Ok(())
    }

    async fn set_session_head(
        &self,
        id: &str,
        head_prompt_turn_id: &str,
    ) -> Result<(), StoreError> {
        let ts = now() as i64;
        let rows = sqlx::query(
            r#"
            UPDATE sessions SET head_prompt_turn_id = ?, updated_at = ? WHERE id = ?
            "#,
        )
        .bind(head_prompt_turn_id)
        .bind(ts)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| map_sqlx_error("session", id, e))?
        .rows_affected();

        if rows == 0 {
            return Err(StoreError::NotFound {
                entity: "session",
                id: id.to_string(),
            });
        }
        Ok(())
    }

    async fn append_prompt_turn(&self, turn: PromptTurn) -> Result<(), StoreError> {
        {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?)",
            )
            .bind(&turn.session_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

            if !exists {
                return Err(StoreError::NotFound {
                    entity: "session",
                    id: turn.session_id.clone(),
                });
            }
        }

        sqlx::query(
            r#"
            INSERT INTO prompt_turns (id, session_id, parent_id, position, created_at)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(&turn.id)
        .bind(&turn.session_id)
        .bind(&turn.parent_id)
        .bind(turn.position as i64)
        .bind(turn.created_at as i64)
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(|e| map_sqlx_error("prompt_turn", &turn.id, e))
    }

    async fn get_prompt_turn_children(
        &self,
        id: &str,
    ) -> Result<Vec<PromptTurn>, StoreError> {
        let rows = sqlx::query_as::<_, PromptTurnRow>(
            r#"
            SELECT id, session_id, parent_id, position, created_at
            FROM prompt_turns WHERE parent_id = ?
            ORDER BY position
            "#,
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Database(e.to_string()))?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn get_session_prompt_turns(
        &self,
        session_id: &str,
    ) -> Result<Vec<PromptTurn>, StoreError> {
        let rows = sqlx::query_as::<_, PromptTurnRow>(
            r#"
            SELECT id, session_id, parent_id, position, created_at
            FROM prompt_turns WHERE session_id = ?
            ORDER BY position
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Database(e.to_string()))?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn append_message(&self, message: Message) -> Result<(), StoreError> {
        {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM prompt_turns WHERE id = ?)",
            )
            .bind(&message.prompt_turn_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

            if !exists {
                return Err(StoreError::NotFound {
                    entity: "prompt_turn",
                    id: message.prompt_turn_id.clone(),
                });
            }
        }

        sqlx::query(
            r#"
            INSERT INTO messages (id, prompt_turn_id, role, content, position, created_at)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&message.id)
        .bind(&message.prompt_turn_id)
        .bind(&message.role)
        .bind(&message.content)
        .bind(message.position as i64)
        .bind(message.created_at as i64)
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(|e| map_sqlx_error("message", &message.id, e))
    }

    async fn get_messages_for_turn(&self, turn_id: &str) -> Result<Vec<Message>, StoreError> {
        let rows = sqlx::query_as::<_, MessageRow>(
            r#"
            SELECT id, prompt_turn_id, role, content, position, created_at
            FROM messages WHERE prompt_turn_id = ?
            ORDER BY position
            "#,
        )
        .bind(turn_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Database(e.to_string()))?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn get_context(
        &self,
        session_id: &str,
        max_turns: Option<usize>,
    ) -> Result<Vec<Message>, StoreError> {
        {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?)",
            )
            .bind(session_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

            if !exists {
                return Err(StoreError::NotFound {
                    entity: "session",
                    id: session_id.to_string(),
                });
            }
        }

        let turn_ids: Vec<(String,)> = sqlx::query_as(
            r#"
            SELECT id FROM prompt_turns WHERE session_id = ? ORDER BY position
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Database(e.to_string()))?;

        let turn_ids: Vec<String> = turn_ids.into_iter().map(|r| r.0).collect();
        let relevant: &[String] = match max_turns {
            Some(0) => return Ok(vec![]),
            Some(n) => {
                let len = turn_ids.len();
                let start = len.saturating_sub(n);
                &turn_ids[start..]
            }
            None => &turn_ids,
        };

        let mut result = Vec::new();
        for turn_id in relevant {
            let mut msgs = self.get_messages_for_turn(turn_id).await?;
            result.append(&mut msgs);
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
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?)",
            )
            .bind(source_session_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

            if !exists {
                return Err(StoreError::NotFound {
                    entity: "session",
                    id: source_session_id.to_string(),
                });
            }
        }

        let source_turns = self.get_session_prompt_turns(source_session_id).await?;
        let fork_idx = source_turns
            .iter()
            .position(|t| t.id == fork_point_turn_id)
            .ok_or_else(|| StoreError::NotFound {
                entity: "prompt_turn",
                id: fork_point_turn_id.to_string(),
            })?;

        let mut turn_id_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let new_session_id = new_session.id.clone();

        let mut forked_session = new_session;
        forked_session.forked_from_session_id = Some(source_session_id.to_string());
        forked_session.fork_point_turn_id = Some(fork_point_turn_id.to_string());
        self.create_session(forked_session).await?;

        for turn in &source_turns[..=fork_idx] {
            let new_turn_id = uuid::Uuid::new_v4().to_string();
            turn_id_map.insert(turn.id.clone(), new_turn_id.clone());

            sqlx::query(
                r#"
                INSERT INTO prompt_turns (id, session_id, parent_id, position, created_at)
                VALUES (?, ?, ?, ?, ?)
                "#,
            )
            .bind(&new_turn_id)
            .bind(&new_session_id)
            .bind(&turn.parent_id)
            .bind(turn.position as i64)
            .bind(turn.created_at as i64)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

            let source_msgs = self.get_messages_for_turn(&turn.id).await?;
            for msg in &source_msgs {
                let new_msg_id = uuid::Uuid::new_v4().to_string();
                sqlx::query(
                    r#"
                    INSERT INTO messages (id, prompt_turn_id, role, content, position, created_at)
                    VALUES (?, ?, ?, ?, ?, ?)
                    "#,
                )
                .bind(&new_msg_id)
                .bind(&new_turn_id)
                .bind(&msg.role)
                .bind(&msg.content)
                .bind(msg.position as i64)
                .bind(msg.created_at as i64)
                .execute(&self.pool)
                .await
                .map_err(|e| StoreError::Database(e.to_string()))?;
            }
        }

        if let Some(new_id) = turn_id_map.get(fork_point_turn_id) {
            self.set_session_head(&new_session_id, new_id).await?;
        }

        Ok(())
    }

    async fn clear(&self) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM messages")
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;
        sqlx::query("DELETE FROM prompt_turns")
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;
        sqlx::query("DELETE FROM sessions")
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(|e| StoreError::Database(e.to_string()))
    }
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    id: String,
    prompt_turn_id: String,
    role: String,
    content: String,
    position: i64,
    created_at: i64,
}

impl From<MessageRow> for Message {
    fn from(row: MessageRow) -> Self {
        Message {
            id: row.id,
            prompt_turn_id: row.prompt_turn_id,
            role: row.role,
            content: row.content,
            position: row.position as usize,
            created_at: row.created_at as u64,
        }
    }
}

#[derive(sqlx::FromRow)]
struct PromptTurnRow {
    id: String,
    session_id: String,
    parent_id: Option<String>,
    position: i64,
    created_at: i64,
}

impl From<PromptTurnRow> for PromptTurn {
    fn from(row: PromptTurnRow) -> Self {
        PromptTurn {
            id: row.id,
            session_id: row.session_id,
            parent_id: row.parent_id,
            messages: Vec::new(),
            position: row.position as usize,
            created_at: row.created_at as u64,
        }
    }
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    id: String,
    head_prompt_turn_id: Option<String>,
    forked_from_session_id: Option<String>,
    fork_point_turn_id: Option<String>,
    cwd: String,
    title: String,
    mode: Option<String>,
    created_at: i64,
    updated_at: i64,
    active: i32,
    transport: String,
}

impl From<SessionRow> for Session {
    fn from(row: SessionRow) -> Self {
        Session {
            id: row.id,
            head_prompt_turn_id: row.head_prompt_turn_id,
            cwd: row.cwd,
            title: row.title,
            mode: row.mode,
            prompt_turns: std::collections::VecDeque::new(),
            prompt_turn_count: 0,
            forked_from_session_id: row.forked_from_session_id,
            fork_point_turn_id: row.fork_point_turn_id,
            created_at: row.created_at as u64,
            updated_at: row.updated_at as u64,
            active: row.active != 0,
            transport: row.transport,
        }
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
        let store = SqliteSessionStore::connect(":memory:").await.unwrap();
        run_session_tests(&store).await;
    }

    #[tokio::test]
    async fn test_prompt_turn_ops() {
        let store = SqliteSessionStore::connect(":memory:").await.unwrap();
        run_prompt_turn_tests(&store).await;
    }

    #[tokio::test]
    async fn test_message_ops() {
        let store = SqliteSessionStore::connect(":memory:").await.unwrap();
        run_message_tests(&store).await;
    }

    #[tokio::test]
    async fn test_context_assembly() {
        let store = SqliteSessionStore::connect(":memory:").await.unwrap();
        run_context_tests(&store).await;
    }

    #[tokio::test]
    async fn test_fork() {
        let store = SqliteSessionStore::connect(":memory:").await.unwrap();
        run_fork_tests(&store).await;
    }
}
