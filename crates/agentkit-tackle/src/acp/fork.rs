//! Session forking (T-027): the shared fork operation behind the RFD
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
use crate::scripts::host::register_host_api;
use crate::scripts::host::BehaviourHost;
use crate::store::{
    MessageId, Role, Session, SessionId, SessionKind, SessionStore, StoreError, TurnKind,
};
use std::collections::BTreeMap;
use std::path::Path;

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

/// Resolves the `session_forked` scripts: sources from the user layer's
/// scripts directory; project-layer scripts are skipped until the trust
/// gate wires script loading (the first-run flow) — never loaded
/// untrusted.
pub fn resolve_session_forked_scripts(
    scripts: &BTreeMap<String, ScriptConfig>,
    user_dir: Option<&Path>,
    project_dir: Option<&Path>,
) -> Vec<(String, String)> {
    let mut sources = Vec::new();
    for (name, config) in scripts {
        if !config.enabled || !config.events.iter().any(|event| event == "session_forked") {
            continue;
        }
        if let Some(project_dir) = project_dir {
            let candidate = project_dir.join("scripts").join(&config.file);
            if candidate.exists() {
                tracing::warn!(
                    script = %name,
                    "project script skipped: script trust gating is wired with the first-run flow"
                );
                continue;
            }
        }
        if let Some(source) = crate::builtins::script_source(&config.file) {
            sources.push((name.clone(), source.to_owned()));
            continue;
        }
        let Some(user_dir) = user_dir else {
            continue;
        };
        match std::fs::read_to_string(user_dir.join("scripts").join(&config.file)) {
            Ok(source) => sources.push((name.clone(), source)),
            Err(err) => tracing::warn!(script = %name, "session_forked script unreadable: {err}"),
        }
    }
    sources
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
    let user_dir = state.config.user_layer.as_deref().and_then(Path::parent);
    let project_dir = state.config.project_layer.as_deref().and_then(Path::parent);
    let sources =
        resolve_session_forked_scripts(&state.config.config.scripts, user_dir, project_dir);

    let known_actors: std::collections::BTreeSet<String> =
        state.definitions.actors.keys().cloned().collect();
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
        let mut event = rhai::Map::new();
        event.insert("source_session_id".into(), source_session_id.clone().into());
        event.insert(
            "fork_point_turn_id".into(),
            fork_point_turn_id.cloned().unwrap_or_default().into(),
        );
        let mut session = rhai::Map::new();
        session.insert("id".into(), fork_id.clone().into());
        session.insert("ephemeral".into(), false.into());
        session.insert("title".into(), "".into());
        event.insert("session".into(), session.into());

        if let Err(err) = compiled.call("session_forked", vec![event.into()]) {
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
    let text = format!("Forked from session {source_id}. This session works in its own copy of the conversation; the parent stays available.");
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
