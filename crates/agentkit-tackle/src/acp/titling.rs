//! Session titling: the default `title_trigger` behaviour.
//!
//! The shipped titling script (builtins::TITLING_SCRIPT) forks an
//! ephemeral summariser, has it summarise the conversation into a
//! six-word title, and sets it via `session_info_update` — the store
//! title update the ACP layer surfaces. Failures are silent and never
//! block: the firing runs detached from the turn and every script error
//! only logs. Only untitled sessions trigger titling.

use crate::acp::TackleState;
use crate::acp::script_loop;
use crate::scripts::host::{BehaviourHost, SessionAccess, block_on};
use crate::scripts::{behaviour_engine, host::register_host_api};
use crate::store::SessionId;
use std::path::Path;
use std::sync::Arc;

/// Fires the `title_trigger` event on the registered titling scripts.
/// Failures are silent: logged, never propagated, never blocking.
pub fn fire_title_trigger(
    state: &TackleState,
    access: Arc<dyn SessionAccess>,
    session_id: &SessionId,
) {
    let config = state.config();
    let user_dir = config.user_layer.as_deref().and_then(Path::parent);
    let known_actors: std::collections::BTreeSet<String> =
        state.definitions().actors.keys().cloned().collect();

    for (name, source) in titling_sources(state, user_dir) {
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
        let event = serde_json::json!({
            "session": { "id": session_id, "ephemeral": ephemeral },
        });

        if let Err(err) = compiled.call("title_trigger", &event) {
            tracing::debug!(script = %name, "title_trigger failed: {err}");
        }
    }
}

/// Resolves the titling scripts: the registrations declaring the
/// `title_trigger` event (the shipped `titling` registration at the
/// lowest precedence, replaceable wholesale by a same-name entry), each
/// from its own layer through the shared resolver.
fn titling_sources(state: &TackleState, user_dir: Option<&Path>) -> Vec<(String, String)> {
    state
        .config()
        .config
        .scripts
        .iter()
        .filter(|(_, config)| {
            config.enabled && config.events.iter().any(|event| event == "title_trigger")
        })
        .filter_map(|(name, config)| {
            crate::config::resolve_script_source(name, &config.file, user_dir)
                .map(|source| (name.clone(), source))
        })
        .collect()
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
        let access = script_loop::access(&state, window_size);
        fire_title_trigger(&state, access, &session_id);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::script_loop::SharedCompletions;
    use crate::scripts::host::PromptDispatcher;
    use crate::scripts::host::{
        Completion, CompletionSink, LoopPendingCompletions, LoopPendingDispatcher, StoreAccess,
    };
    use crate::store::{SessionKind, SessionStore};
    use std::collections::BTreeSet;
    use std::sync::Mutex;

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
        // The shipped registration production gets from
        // `config::builtin_registrations`: without it no titling script
        // fires, exactly as in production when it is disabled.
        let mut config = crate::config::Loaded::default();
        config.config.scripts.insert(
            "titling".to_owned(),
            crate::config::ScriptConfig {
                events: vec!["title_trigger".to_owned()],
                file: "builtin:titling.rhai".to_owned(),
                enabled: true,
            },
        );
        Arc::new(TackleState::new(
            Arc::clone(db),
            config,
            crate::loader::Definitions::default().with_builtins(),
        ))
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

        let state = Arc::new(TackleState::new(
            Arc::clone(&db),
            config,
            crate::loader::Definitions::default().with_builtins(),
        ));
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
    fn disabling_the_built_in_titling_turns_it_off() {
        // Symmetric with compaction: a disabled registration means no
        // script fires, not a silent reversion to the shipped default.
        let (db, origin, _dispatcher) = fixture("A Six Word Title");
        let mut config = crate::config::Loaded::default();
        config.config.scripts.insert(
            "titling".to_owned(),
            crate::config::ScriptConfig {
                events: vec!["title_trigger".to_owned()],
                file: "builtin:titling.rhai".to_owned(),
                enabled: false,
            },
        );
        let state = Arc::new(TackleState::new(
            Arc::clone(&db),
            config,
            crate::loader::Definitions::default().with_builtins(),
        ));
        let access: Arc<dyn SessionAccess> = Arc::new(StoreAccess::new(
            Arc::clone(&db),
            Arc::new(LoopPendingDispatcher),
            Arc::new(LoopPendingCompletions),
            100_000,
        ));
        fire_title_trigger(&state, access, &origin);

        let session = block_on(db.get_session(&origin)).unwrap().unwrap();
        assert_eq!(session.title, "", "a disabled titling script never fires");
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
