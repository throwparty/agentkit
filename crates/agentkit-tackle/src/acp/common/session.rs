//! Lease acquisition and prompt-turn persistence, shared by the version
//! modules. Nothing here depends on a protocol version's wire types: the user
//! message arrives already serialised.

use super::content::{self, Block};
use crate::agent::config_options::{self, Selectors};
use crate::permissions::PermissionRequester;
use crate::state::{LeaseHeartbeat, TackleState, LEASE_TTL_SECS};
use crate::store::{Message, Role, Session, SessionId, SessionKind, Turn, TurnKind, TurnUsage};
use std::sync::Arc;

/// The persisted state of a begun prompt turn, plus the lease heartbeat that
/// must outlive it.
pub(crate) struct BegunTurn {
    pub store_session: SessionId,
    pub turn: Turn,
    pub blocks_json: String,
    pub heartbeat: LeaseHeartbeat,
    /// The persisted user message's id: the v2 acceptance receipt.
    pub user_message_id: String,
}

/// Why a prompt turn could not begin.
pub(crate) enum BeginError {
    /// Another connection holds the session's turn lease.
    Owned { holder: String },
    /// The store failed while acquiring the lease or persisting the turn.
    Store(String),
}

/// Acquires the one-turn-at-a-time lease and persists the prompt turn and its
/// user message. `blocks_json` is the user message content already serialised
/// in the caller's wire form. The returned heartbeat must be held for the
/// turn's lifetime; dropping it stops renewal.
pub(crate) async fn begin_prompt_turn(
    state: &TackleState,
    store_session: SessionId,
    blocks: Vec<Block>,
    owner: &str,
) -> Result<BegunTurn, BeginError> {
    let blocks_json = content::encode(&blocks);
    let acquired = {
        let mut acquired = false;
        for _ in 0..4 {
            acquired = state
                .db
                .acquire_lease(&store_session, owner, LEASE_TTL_SECS)
                .await
                .map_err(|err| BeginError::Store(err.to_string()))?;
            if acquired {
                break;
            }
            // The lease can be released between the failed acquire and the
            // holder lookup; retry instead of reporting a phantom owner.
            match state.db.lease_holder(&store_session).await {
                Ok(Some(holder)) => return Err(BeginError::Owned { holder }),
                Ok(None) => tokio::task::yield_now().await,
                Err(err) => return Err(BeginError::Store(err.to_string())),
            }
        }
        acquired
    };
    if !acquired {
        return Err(BeginError::Owned {
            holder: "another connection".into(),
        });
    }
    let heartbeat = LeaseHeartbeat::spawn(
        std::sync::Arc::clone(&state.db),
        store_session.clone(),
        owner.to_owned(),
    );
    let head = state
        .db
        .get_session(&store_session)
        .await
        .ok()
        .flatten()
        .and_then(|session| session.head_turn_id);
    let turn = state
        .db
        .append_turn(
            &store_session,
            head.as_ref(),
            TurnKind::Interaction,
            None,
            TurnUsage::default(),
        )
        .await
        .map_err(|err| BeginError::Store(err.to_string()))?;
    let user_message = state
        .db
        .append_message(&turn.id, Role::User, &blocks_json, None, None, None, None)
        .await
        .map_err(|err| BeginError::Store(err.to_string()))?;
    Ok(BegunTurn {
        store_session,
        turn,
        blocks_json,
        heartbeat,
        user_message_id: user_message.id,
    })
}

/// The session-creation metadata: the selected actor plus any
/// version-specific keys (v2's `additionalDirectories`). Kept pure so the
/// merge is testable without a store.
pub(crate) fn session_metadata(
    actor: Option<&str>,
    extra: Option<serde_json::Value>,
) -> String {
    let mut metadata = serde_json::json!({
        "actor": actor.unwrap_or("default"),
    });
    if let (Some(metadata), Some(extra)) = (
        metadata.as_object_mut(),
        extra.as_ref().and_then(serde_json::Value::as_object),
    ) {
        for (key, value) in extra {
            metadata.insert(key.clone(), value.clone());
        }
    }
    metadata.to_string()
}

/// Creates the interactive store session. Project trust, the first-run
/// seed, and the selectors follow in [`resolve_project_trust`],
/// [`seed_first_turn`], and [`build_selectors`].
pub(crate) async fn create_interactive(
    state: &TackleState,
    cwd: &str,
    extra_metadata: Option<serde_json::Value>,
) -> Result<Session, String> {
    let actor = state.config().config.defaults.actor.clone();
    state
        .db
        .create_session(
            SessionKind::Interactive,
            cwd,
            None,
            None,
            &session_metadata(actor.as_deref(), extra_metadata),
        )
        .await
        .map_err(|err| err.to_string())
}

/// Resolves pending project trust for a new session on the connection's
/// MCP pool. Consent goes back to the client, so the resolution runs off
/// the dispatch task; awaiting it here would deadlock on the connection's
/// own response routing. The version supplies its own requester.
pub(crate) async fn resolve_project_trust<R>(
    state: Arc<TackleState>,
    pool: Arc<tokio::sync::Mutex<crate::mcp::McpPool>>,
    requester: R,
    session_id: String,
) where
    R: PermissionRequester + 'static,
{
    let resolved = tokio::spawn(async move {
        state
            .resolve_pending_project(Some(pool), &requester, &session_id)
            .await;
    })
    .await;
    if let Err(err) = resolved {
        tracing::error!("project trust resolution failed: {err}");
    }
}

/// Appends the harness-authored first-run seed on its own seed turn and
/// returns the stored message for the post-response announcement.
pub(crate) async fn seed_first_turn(
    state: &TackleState,
    session: &Session,
) -> Result<Message, String> {
    let seed = crate::agent::seed::first_run_seed(state);
    let blocks = [Block::text(seed)];
    let seed_turn = state
        .db
        .append_turn(
            &session.id,
            None,
            TurnKind::Seed,
            None,
            TurnUsage::default(),
        )
        .await
        .map_err(|err| err.to_string())?;
    state
        .db
        .append_message(
            &seed_turn.id,
            Role::User,
            &content::encode(&blocks),
            None,
            None,
            None,
            None,
        )
        .await
        .map_err(|err| err.to_string())
}

/// Builds the response selectors and seeds the effective model into the
/// session metadata when unset, so display and the first prompt agree
/// without an explicit client switch.
pub(crate) async fn build_selectors(
    state: &TackleState,
    session: &Session,
) -> Result<Selectors, String> {
    let mut selectors = config_options::build(state, session).await;
    if crate::store::session_model_free(session).is_none() {
        if let Some(model) =
            config_options::resolve_model_for(state, session, selectors.first_model.as_deref())
        {
            state
                .db
                .set_model(&session.id, &model)
                .await
                .map_err(|err| err.to_string())?;
            if let Ok(Some(fresh)) = state.db.get_session(&session.id).await {
                selectors = config_options::build(state, &fresh).await;
            }
        }
    }
    Ok(selectors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_defaults_the_actor() {
        let metadata: serde_json::Value =
            serde_json::from_str(&session_metadata(None, None)).unwrap();
        assert_eq!(metadata["actor"], "default");
        assert!(metadata.get("additionalDirectories").is_none());
    }

    #[test]
    fn metadata_merges_version_specific_keys() {
        let metadata: serde_json::Value = serde_json::from_str(&session_metadata(
            Some("summariser"),
            Some(serde_json::json!({ "additionalDirectories": ["/tmp/one"] })),
        ))
        .unwrap();
        assert_eq!(metadata["actor"], "summariser");
        assert_eq!(metadata["additionalDirectories"][0], "/tmp/one");
    }
}
