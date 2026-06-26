use crate::session::{RoutingEvent, SessionAffinity, SessionError, SessionManager, SessionStats};
use sqlx::SqlitePool;

pub struct SqliteSessionManager {
    pool: SqlitePool,
}

impl SqliteSessionManager {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl SessionManager for SqliteSessionManager {
    async fn lookup(&self, session_id: &str) -> Result<Option<SessionAffinity>, SessionError> {
        let row = sqlx::query_as::<_, (String, String, String)>(
            "SELECT provider_identity, model_name, api_surface FROM session_affinity WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| SessionError::Database(e.to_string()))?;

        Ok(row.map(
            |(provider_identity, model_name, api_surface)| SessionAffinity {
                session_id: session_id.to_string(),
                provider_identity,
                model_name,
                api_surface,
            },
        ))
    }

    async fn assign(
        &self,
        session_id: &str,
        provider: &str,
        model: &str,
        surface: &str,
    ) -> Result<(), SessionError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        sqlx::query(
            "INSERT INTO session_affinity (session_id, provider_identity, model_name, api_surface, assigned_at, last_used_at) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT(session_id) DO UPDATE SET \
             provider_identity = excluded.provider_identity, \
             model_name = excluded.model_name, \
             api_surface = excluded.api_surface, \
             last_used_at = excluded.last_used_at",
        )
        .bind(session_id)
        .bind(provider)
        .bind(model)
        .bind(surface)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::Database(e.to_string()))?;

        Ok(())
    }

    async fn update_tokens(
        &self,
        session_id: &str,
        input: u64,
        output: u64,
    ) -> Result<(), SessionError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        sqlx::query(
            "UPDATE session_affinity SET \
             total_input_tokens = total_input_tokens + ?, \
             total_output_tokens = total_output_tokens + ?, \
             total_requests = total_requests + 1, \
             last_used_at = ? \
             WHERE session_id = ?",
        )
        .bind(input as i64)
        .bind(output as i64)
        .bind(now)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::Database(e.to_string()))?;

        Ok(())
    }

    async fn increment_switch(
        &self,
        session_id: &str,
        new_provider: &str,
    ) -> Result<(), SessionError> {
        sqlx::query(
            "UPDATE session_affinity SET \
             provider_identity = ?, \
             switch_count = switch_count + 1 \
             WHERE session_id = ?",
        )
        .bind(new_provider)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::Database(e.to_string()))?;

        Ok(())
    }

    async fn insert_routing_event(&self, event: RoutingEvent) -> Result<(), SessionError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        sqlx::query(
            "INSERT INTO routing_events \
             (session_id, request_id, model_name, provider_identity, billing_model, \
              decision_reason, input_tokens, output_tokens, response_status, latency_ms, \
              degraded_providers, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&event.session_id)
        .bind(&event.request_id)
        .bind(&event.model_name)
        .bind(&event.provider_identity)
        .bind(&event.billing_model)
        .bind(&event.decision_reason)
        .bind(event.input_tokens)
        .bind(event.output_tokens)
        .bind(event.response_status)
        .bind(event.latency_ms)
        .bind(&event.degraded_providers)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::Database(e.to_string()))?;

        Ok(())
    }

    async fn stats(&self) -> Result<SessionStats, SessionError> {
        let (active_sessions, total_sessions) = sqlx::query_as::<_, (i64, i64)>(
            "SELECT \
             COALESCE(SUM(CASE WHEN is_active = 1 THEN 1 ELSE 0 END), 0), \
             COUNT(*) \
             FROM session_affinity",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| SessionError::Database(e.to_string()))?;

        Ok(SessionStats {
            active_sessions: active_sessions as u64,
            total_sessions: total_sessions as u64,
        })
    }
}
