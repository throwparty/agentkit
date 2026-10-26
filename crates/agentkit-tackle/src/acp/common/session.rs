//! Lease acquisition and prompt-turn persistence, shared by the version
//! modules. Nothing here depends on a protocol version's wire types: the user
//! message arrives already serialised.

use crate::state::{LeaseHeartbeat, TackleState, LEASE_TTL_SECS};
use crate::store::{Role, SessionId, Turn, TurnKind, TurnUsage};

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
    blocks_json: String,
    owner: &str,
) -> Result<BegunTurn, BeginError> {
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
