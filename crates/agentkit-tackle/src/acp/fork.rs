//! Session forking: the shared fork operation behind the RFD
//! path and the v1 `/fork` fallback.
//!
//! Forks copy lineage (the source session and its head as the fork
//! point) and are titled from their parent at creation — derived, not
//! model-generated. The `session_forked` script event fires with the
//! full behaviour host API; a script error degrades gracefully: logged,
//! never failing the fork. Seeds are harness-authored static
//! user-facing content on their own turn kind. Updates flow only after
//! the client attaches — the fork handlers perform no session updates.

use crate::acp::TackleState;
use crate::config::ScriptConfig;
use crate::scripts::behaviour_engine;
use crate::scripts::host::BehaviourHost;
use crate::scripts::host::register_host_api;
use crate::store::{
    MessageId, Role, Session, SessionId, SessionKind, SessionStore, StoreError, TurnKind,
};
use std::collections::BTreeMap;

pub use crate::scripts::host::{Completion, CompletionSink, HostError, PromptDispatcher};
pub use crate::scripts::host::{SessionAccess, StoreAccess};

/// Creates the fork: lineage and fork point from the source, title
/// derived from the parent.
pub async fn create_fork(db: &SessionStore, source_id: &SessionId) -> Result<Session, StoreError> {
    let source = db
        .get_session(source_id)
        .await?
        .ok_or_else(|| StoreError::NotFound(source_id.clone()))?;
    let fork = db
        .create_session(
            SessionKind::Interactive,
            &source.cwd,
            Some(source_id),
            source.head_turn_id.as_ref(),
            &source.metadata,
        )
        .await?;
    // Forks are titled from their parent at creation.
    db.set_title(&fork.id, &source.title).await?;
    Ok(Session {
        title: source.title,
        ..fork
    })
}

/// Resolves the `session_forked` scripts from the cached bodies (loaded
/// once), each from its own layer through the shared resolver.
pub fn resolve_session_forked_scripts(
    scripts: &BTreeMap<String, ScriptConfig>,
    sources: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    scripts
        .iter()
        .filter(|(_, config)| {
            config.enabled && config.events.iter().any(|event| event == "session_forked")
        })
        .filter_map(|(name, _)| sources.get(name).map(|source| (name.clone(), source.clone())))
        .collect()
}

/// Fires the `session_forked` event on every registered script. A
/// script error never fails the fork.
pub fn fire_session_forked(
    state: &TackleState,
    access: std::sync::Arc<dyn SessionAccess>,
    fork_id: &SessionId,
    source_session_id: &SessionId,
    fork_point_turn_id: Option<&crate::store::TurnId>,
) {
    let config = state.config();
    let sources = resolve_session_forked_scripts(&config.config.scripts, &state.script_sources());

    let known_actors: std::collections::BTreeSet<String> =
        state.definitions().actors.keys().cloned().collect();
    for (name, source) in sources {
        let host = BehaviourHost::new(
            std::sync::Arc::clone(&access),
            known_actors.clone(),
            fork_id.clone(),
            name.clone(),
        );
        let compiled = match behaviour_engine(&source, &|engine| {
            register_host_api(engine, std::sync::Arc::clone(&host))
        }) {
            Ok(compiled) => compiled,
            Err(err) => {
                tracing::warn!(script = %name, "session_forked script failed to compile: {err}");
                continue;
            }
        };
        // The event payload, per scripts-api.md.
        let event = serde_json::json!({
            "source_session_id": source_session_id,
            "fork_point_turn_id": fork_point_turn_id.cloned().unwrap_or_default(),
            "session": {
                "id": fork_id,
                "ephemeral": false,
                "title": "",
            },
        });

        if let Err(err) = compiled.call("session_forked", &event) {
            tracing::warn!(script = %name, "session_forked script failed: {err}");
        }
    }
}

/// The fallback path's harness-authored seed: static user-facing
/// content with its own message id, on a seed turn.
pub async fn insert_fork_seed(
    db: &SessionStore,
    fork_id: &SessionId,
    source_id: &SessionId,
) -> Result<MessageId, StoreError> {
    let turn = db
        .append_turn(fork_id, None, TurnKind::Seed, None, Default::default())
        .await?;
    let text = format!(
        "Forked from session {source_id}. This session works in its own copy of the conversation; the parent stays available."
    );
    let content = serde_json::json!([{ "type": "text", "text": text }]).to_string();
    let message = db
        .append_message(&turn.id, Role::User, &content, None, None, None, None)
        .await?;
    Ok(message.id)
}

/// The fallback path's parent-turn report: an agent message carrying
/// the new session id, stored in the parent's turn.
pub fn fork_report_message(
    fork_id: &SessionId,
    fork_point: Option<&crate::store::TurnId>,
) -> String {
    match fork_point {
        Some(point) => format!(
            "Forked this conversation to session {fork_id}, continuing from turn {point}. \
             This session stays available; new messages belong in the fork."
        ),
        None => format!(
            "Forked this conversation to session {fork_id}. \
             This session stays available; new messages belong in the fork."
        ),
    }
}

/// Everything the RFD and fallback paths need, shared: fork creation,
/// the script event, and the seed. `source` is the session being
/// forked.
pub async fn run_fork(
    state: &TackleState,
    access: std::sync::Arc<dyn SessionAccess>,
    source_id: &SessionId,
) -> Result<Session, StoreError> {
    let fork = create_fork(&state.db, source_id).await?;
    fire_session_forked(
        state,
        access,
        &fork.id,
        source_id,
        fork.fork_point_turn_id.as_ref(),
    );
    Ok(fork)
}
