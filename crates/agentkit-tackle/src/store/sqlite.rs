//! SQLite row mapping and file-permission helpers.

use super::model::{AssembledMessage, Message, Role, Session, SessionKind, StoreError, TurnKind};
use std::path::Path;

/// Restricts a file's permissions to 0600 on Unix; no-op elsewhere.
pub(super) fn restrict_permissions(path: &Path) -> Result<(), StoreError> {
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
pub(super) struct SqliteSession {
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
    pub(super) fn into_domain(self) -> Session {
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
pub(super) struct SqliteAssembledRow {
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
    pub(super) fn into_domain(self) -> AssembledMessage {
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
