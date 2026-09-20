//! Session titling (T-029): the default `title_trigger` behaviour.
//!
//! The shipped titling script (builtins::TITLING_SCRIPT) forks an
//! ephemeral summariser, has it summarise the conversation into a
//! six-word title, and sets it via `session_info_update` — the store
//! title update the ACP layer surfaces. Failures are silent and never
//! block: the firing runs detached from the turn and every script error
//! only logs. Only untitled sessions trigger titling.

use crate::acp::TackleState;
use crate::scripts::host::{
    block_on, BehaviourHost, Completion, CompletionSink, PromptDispatcher, SessionAccess,
    StoreAccess,
};
use crate::scripts::{behaviour_engine, host::register_host_api};
use crate::store::SessionId;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// A shared completion registry: the sink the access awaits from and
/// the dispatcher publishes into.
#[derive(Default)]
pub struct SharedCompletions {
    map: Mutex<HashMap<SessionId, Completion>>,
}

impl SharedCompletions {
    pub fn new() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
        }
    }
}

impl CompletionSink for SharedCompletions {
    fn publish(&self, session: &SessionId, completion: Completion) {
        self.map.lock().unwrap().insert(session.clone(), completion);
    }

    fn take(&self, session: &SessionId) -> Option<Completion> {
        self.map.lock().unwrap().remove(session)
    }
}

/// Fires the `title_trigger` event on the registered titling scripts.
/// Failures are silent: logged, never propagated, never blocking.
pub fn fire_title_trigger(
    state: &TackleState,
    access: Arc<dyn SessionAccess>,
    session_id: &SessionId,
) {
    let user_dir = state.config.user_layer.as_deref().and_then(Path::parent);
    let project_dir = state.config.project_layer.as_deref().and_then(Path::parent);
    let known_actors: std::collections::BTreeSet<String> =
        state.definitions.actors.keys().cloned().collect();

    for (name, source) in titling_sources(state, user_dir, project_dir) {
        let host = BehaviourHost::new(
            Arc::clone(&access),
            known_actors.clone(),
            session_id.clone(),
            name.clone(),
        );
        let compiled = match behaviour_engine(&source, &|engine| {
            register_host_api(engine, Arc::clone(&host))
        }) {
            Ok(compiled) => compiled,
            Err(err) => {
                tracing::debug!(script = %name, "title_trigger script failed to compile: {err}");
                continue;
            }
        };

        // The event payload: the session (its kind guards the script's
        // own re-entrancy inside ephemeral forks).
        let ephemeral = matches!(
            block_on(state.db.get_session(session_id)),
            Ok(Some(session)) if session.kind == crate::store::SessionKind::Ephemeral
        );
        let mut event = rhai::Map::new();
        let mut session = rhai::Map::new();
        session.insert("id".into(), session_id.clone().into());
        session.insert("ephemeral".into(), ephemeral.into());
        event.insert("session".into(), session.into());

        if let Err(err) = compiled.call("title_trigger", vec![event.into()]) {
            eprintln!("tackle-debug: title_trigger failed: {err}");
        }
    }
}

/// Resolves the titling scripts: the shipped built-in registration
/// unless user configuration overrides it (same-name entries win);
/// project-layer scripts wait for the trust gate (wired with the
/// first-run flow).
fn titling_sources(
    state: &TackleState,
    user_dir: Option<&Path>,
    project_dir: Option<&Path>,
) -> Vec<(String, String)> {
    let mut sources = Vec::new();
    if !state
        .config
        .config
        .scripts
        .values()
        .any(|config| config.events.iter().any(|event| event == "title_trigger"))
    {
        // No titling registration anywhere: the shipped default.
        sources.push((
            "titling".to_owned(),
            crate::builtins::TITLING_SCRIPT.to_owned(),
        ));
    }
    for (name, config) in &state.config.config.scripts {
        if !config.enabled || !config.events.iter().any(|event| event == "title_trigger") {
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
        let Some(user_dir) = user_dir else { continue };
        match std::fs::read_to_string(user_dir.join("scripts").join(&config.file)) {
            Ok(source) => sources.push((name.clone(), source)),
            Err(err) => tracing::debug!(script = %name, "title_trigger script unreadable: {err}"),
        }
    }
    sources
}

/// Detached titling at turn end: only untitled sessions, failures
/// silent, never blocking the response.
pub fn fire_title_trigger_detached(
    state: Arc<TackleState>,
    session_id: SessionId,
    window_size: u64,
) {
    tokio::spawn(async move {
        // Only untitled sessions: the title_trigger summarisation runs
        // once.
        let title = state
            .db
            .get_session(&session_id)
            .await
            .ok()
            .flatten()
            .map(|session| session.title);
        if title.is_some_and(|title| !title.is_empty()) {
            return;
        }
        let access: Arc<dyn SessionAccess> = Arc::new(StoreAccess::new(
            Arc::clone(&state.db),
            Arc::new(LoopPendingDispatcher),
            Arc::new(LoopPendingCompletions),
            window_size,
        ));
        fire_title_trigger(&state, access, &session_id);
    });
}

struct LoopPendingDispatcher;
impl PromptDispatcher for LoopPendingDispatcher {
    fn dispatch(
        &self,
        _session: &SessionId,
        _text: &str,
    ) -> Result<(), crate::scripts::host::HostError> {
        // The agent loop integration drives these model turns; the user
        // message is already stored, so nothing is lost.
        Ok(())
    }
}

struct LoopPendingCompletions;
impl CompletionSink for LoopPendingCompletions {
    fn publish(&self, _session: &SessionId, _completion: Completion) {}
    fn take(&self, _session: &SessionId) -> Option<Completion> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{SessionKind, SessionStore};
    use std::collections::BTreeSet;

    /// The summariser model, stubbed: every dispatched prompt completes
    /// instantly with the fixture title.
    struct AutoCompleting {
        sink: Arc<SharedCompletions>,
        title: String,
        dispatched: Mutex<Vec<SessionId>>,
    }

    impl AutoCompleting {
        fn sink(&self) -> Arc<SharedCompletions> {
            Arc::clone(&self.sink)
        }
    }

    impl PromptDispatcher for AutoCompleting {
        fn dispatch(
            &self,
            session: &SessionId,
            _text: &str,
        ) -> Result<(), crate::scripts::host::HostError> {
            self.dispatched.lock().unwrap().push(session.clone());
            self.sink.publish(
                session,
                Completion {
                    stop_reason: "end_turn".to_owned(),
                    final_message: self.title.clone(),
                    input_tokens: 1,
                    output_tokens: 1,
                },
            );
            Ok(())
        }
    }

    fn fixture(title: &str) -> (Arc<SessionStore>, String, Arc<AutoCompleting>) {
        let store = Arc::new(SessionStore::in_memory());
        let origin = block_on(store.create_session(
            SessionKind::Interactive,
            "/work",
            None,
            None,
            r#"{"actor": "default"}"#,
        ))
        .unwrap()
        .id;
        let shared = Arc::new(SharedCompletions::new());
        let dispatcher = Arc::new(AutoCompleting {
            sink: Arc::clone(&shared),
            title: title.to_owned(),
            dispatched: Mutex::new(Vec::new()),
        });
        (store, origin, dispatcher)
    }

    fn tackle_state(db: &Arc<SessionStore>) -> Arc<TackleState> {
        Arc::new(TackleState {
            db: Arc::clone(db),
            config: crate::config::Loaded::default(),
            definitions: crate::loader::Definitions::default().with_builtins(),
        })
    }

    #[test]
    fn the_titling_flow_sets_a_six_word_title() {
        let (db, origin, dispatcher) = fixture("A Six Word Title");
        let state = tackle_state(&db);
        let access: Arc<dyn SessionAccess> = Arc::new(StoreAccess::new(
            Arc::clone(&db),
            Arc::clone(&dispatcher) as Arc<dyn PromptDispatcher>,
            dispatcher.sink(),
            100_000,
        ));

        fire_title_trigger(&state, access, &origin);

        // The title is set in the store, surfaced as session_info_update
        // by the ACP layer.
        let session = block_on(state.db.get_session(&origin)).unwrap().unwrap();
        assert_eq!(session.title, "A Six Word Title");

        // The summarisation ran in an ephemeral fork of the origin.
        let dispatched = dispatcher.dispatched.lock().unwrap().clone();
        assert_eq!(dispatched.len(), 1);
        assert_ne!(dispatched[0], origin, "the fork is not the origin itself");
        let fork = block_on(state.db.get_session(&dispatched[0]))
            .unwrap()
            .unwrap();
        assert_eq!(fork.kind, SessionKind::Ephemeral);
        assert_eq!(
            fork.forked_from_session_id.as_deref(),
            Some(origin.as_str())
        );
    }

    #[test]
    fn titling_failures_are_silent() {
        // A broken titling script: the firing logs and returns — no
        // error, no store change, no fork.
        let (db, origin, _dispatcher) = fixture("A Six Word Title");
        let mut config = crate::config::Loaded::default();
        config.config.scripts.insert(
            "broken-titling".to_owned(),
            crate::config::ScriptConfig {
                events: vec!["title_trigger".to_owned()],
                file: "broken.rhai".to_owned(),
                enabled: true,
            },
        );

        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("scripts")).unwrap();
        std::fs::write(
            temp.path().join("scripts").join("broken.rhai"),
            "not rhai {",
        )
        .unwrap();
        config.user_layer = Some(temp.path().join("config.toml"));

        let state = Arc::new(TackleState {
            db: Arc::clone(&db),
            config,
            definitions: crate::loader::Definitions::default().with_builtins(),
        });
        let access: Arc<dyn SessionAccess> = Arc::new(StoreAccess::new(
            Arc::clone(&db),
            Arc::new(LoopPendingDispatcher),
            Arc::new(LoopPendingCompletions),
            100_000,
        ));
        fire_title_trigger(&state, access, &origin);

        let session = block_on(db.get_session(&origin)).unwrap().unwrap();
        assert_eq!(session.title, "", "silent failure: nothing set");
    }

    #[test]
    fn ephemeral_sessions_never_re_enter_titling() {
        let (db, origin, _dispatcher) = fixture("A Six Word Title");
        let state = tackle_state(&db);
        // An ephemeral fork of the origin: the script's guard returns
        // before forking.
        let ephemeral_id = block_on(state.db.create_session(
            SessionKind::Ephemeral,
            "/work",
            Some(&origin),
            None,
            "{}",
        ))
        .unwrap()
        .id;
        let access: Arc<dyn SessionAccess> = Arc::new(StoreAccess::new(
            Arc::clone(&db),
            Arc::new(LoopPendingDispatcher),
            Arc::new(LoopPendingCompletions),
            100_000,
        ));
        fire_title_trigger(&state, access, &ephemeral_id);

        let ephemeral_now = block_on(state.db.list_sessions(&crate::store::ListFilter {
            include_ephemeral: true,
            ..Default::default()
        }))
        .unwrap();
        // No NEW ephemeral fork: the only ephemeral session is the one
        // the fixture created (the guard tripped).
        let ephemeral_count = ephemeral_now
            .iter()
            .filter(|session| session.kind == SessionKind::Ephemeral)
            .count();
        assert_eq!(
            ephemeral_count, 1,
            "no fork spawned from an ephemeral session"
        );
    }

    #[test]
    fn known_actors_include_the_built_in_summariser() {
        let (_state, _origin, _dispatcher) = fixture("x");
        let builtins: BTreeSet<String> = crate::builtins::defaults()
            .filter(|(kind, _, _)| *kind == "actor")
            .map(|(_, name, _)| name.to_owned())
            .collect();
        assert!(builtins.contains("summariser"));
        assert!(builtins.contains("default"));
    }
}
