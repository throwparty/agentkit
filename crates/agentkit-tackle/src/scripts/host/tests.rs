//! Behaviour host tests: budget enforcement, host-function behaviour, and payload caps.

    use super::*;
    use crate::store::{SessionId, SessionKind, SessionStore, TurnKind, TurnUsage};
    use std::collections::{BTreeSet, HashMap};
    use std::sync::{Arc, Mutex};

    struct RecordingDispatcher {
        prompts: Mutex<Vec<(SessionId, String)>>,
    }

    impl PromptDispatcher for RecordingDispatcher {
        fn dispatch(&self, session: &SessionId, text: &str) -> Result<(), HostError> {
            self.prompts
                .lock()
                .unwrap()
                .push((session.clone(), text.to_owned()));
            Ok(())
        }
    }

    struct RegistryCompletions {
        completions: Mutex<HashMap<SessionId, Completion>>,
    }

    impl CompletionSink for RegistryCompletions {
        fn publish(&self, session: &SessionId, completion: Completion) {
            self.completions
                .lock()
                .unwrap()
                .insert(session.clone(), completion);
        }

        fn take(&self, session: &SessionId) -> Option<Completion> {
            self.completions.lock().unwrap().remove(session)
        }
    }

    struct Fixture {
        access: Arc<StoreAccess>,
        dispatcher: Arc<RecordingDispatcher>,
        completions: Arc<RegistryCompletions>,
        origin: SessionId,
    }

    fn fixture() -> Fixture {
        let store = Arc::new(SessionStore::in_memory());
        let origin = block_on(store.create_session(
            SessionKind::Interactive,
            "/workspace",
            None,
            None,
            "{}",
        ))
        .unwrap()
        .id;
        let dispatcher = Arc::new(RecordingDispatcher {
            prompts: Mutex::new(Vec::new()),
        });
        let completions = Arc::new(RegistryCompletions {
            completions: Mutex::new(HashMap::new()),
        });
        let access = Arc::new(StoreAccess::new(
            store,
            Arc::clone(&dispatcher) as Arc<dyn PromptDispatcher>,
            Arc::clone(&completions) as Arc<dyn CompletionSink>,
            100_000,
        ));
        Fixture {
            access,
            dispatcher,
            completions,
            origin,
        }
    }

    fn host(fixture: &Fixture) -> Arc<BehaviourHost> {
        BehaviourHost::new(
            Arc::clone(&fixture.access) as Arc<dyn SessionAccess>,
            BTreeSet::from(["summariser".to_owned()]),
            fixture.origin.clone(),
            "test-script".to_owned(),
        )
    }

    #[test]
    fn fork_session_creates_the_fork_and_enforces_the_depth_one_rule() {
        let fixture = fixture();
        let host = host(&fixture);

        let fork_id = host.fork_session(true, "summariser").unwrap();
        let fork = block_on(fixture.access.store.get_session(&fork_id))
            .unwrap()
            .unwrap();
        assert_eq!(fork.kind, SessionKind::Ephemeral);
        assert_eq!(
            fork.forked_from_session_id.as_deref(),
            Some(fixture.origin.as_str())
        );

        // An unknown actor is a script error.
        let err = host.fork_session(true, "rogue").unwrap_err();
        assert!(err.to_string().contains("unknown actor `rogue`"), "{err}");

        // Depth one: a script inside an ephemeral fork cannot fork again.
        let nested_host = BehaviourHost::new(
            Arc::clone(&fixture.access) as Arc<dyn SessionAccess>,
            BTreeSet::new(),
            fork_id.clone(),
            "test-script".to_owned(),
        );
        let err = nested_host.fork_session(false, "").unwrap_err();
        assert!(
            err.to_string().contains("nesting beyond depth one"),
            "{err}"
        );
    }

    #[test]
    fn live_ephemeral_forks_are_budget_capped() {
        let fixture = fixture();
        let host = host(&fixture);

        for _ in 0..MAX_LIVE_EPHEMERAL_FORKS {
            host.fork_session(true, "").unwrap();
        }
        let err = host.fork_session(true, "").unwrap_err();
        assert!(
            err.to_string()
                .contains("budget exhausted: live ephemeral forks"),
            "{err}"
        );
    }

    #[test]
    fn send_prompt_dispatches_records_and_enforces_budgets() {
        let fixture = fixture();
        let host = host(&fixture);
        let target = host.fork_session(true, "").unwrap();

        host.send_prompt(&target, "do the thing").unwrap();
        assert_eq!(
            fixture.dispatcher.prompts.lock().unwrap().clone(),
            vec![(target.clone(), "do the thing".to_owned())]
        );
        // The prompt entered the target session as a user message.
        let assembled = block_on(fixture.access.store.assemble_context(&target)).unwrap();
        assert!(
            assembled
                .iter()
                .any(|item| item.message.content.contains("do the thing"))
        );

        // Budget exhaustion: prompts per script.
        host.send_prompt(&target, "again").unwrap();
        let err = host.send_prompt(&target, "once more").unwrap_err();
        assert!(
            err.to_string()
                .contains("budget exhausted: prompts per script"),
            "{err}"
        );
    }

    #[test]
    fn the_cost_cap_is_enforced_at_the_parent() {
        let fixture = fixture();
        let host = host(&fixture);

        // Attribute cost above the cap to the parent.
        let turn = block_on(fixture.access.store.append_turn(
            &fixture.origin,
            None,
            TurnKind::Interaction,
            None,
            Default::default(),
        ))
        .unwrap();
        block_on(fixture.access.store.set_turn_usage(
            &turn.id,
            TurnUsage {
                input_tokens: 1,
                output_tokens: 1,
                cost_usd: PARENT_COST_CAP_USD + 0.01,
            },
        ))
        .unwrap();

        let target = host.fork_session(true, "").unwrap();
        let err = host.send_prompt(&target, "spend more").unwrap_err();
        assert!(
            err.to_string()
                .contains("budget exhausted: parent cost cap"),
            "{err}"
        );
    }

    #[test]
    fn fork_usage_attributes_to_the_parent() {
        let fixture = fixture();
        let host = host(&fixture);
        let fork = host.fork_session(true, "").unwrap();

        assert_eq!(
            fixture.access.attributed_cost(&fixture.origin).unwrap(),
            0.0
        );

        // The fork's model calls land on its turns; the parent's
        // attributable cost includes them.
        let turn = block_on(fixture.access.store.append_turn(
            &fork,
            None,
            TurnKind::Interaction,
            None,
            Default::default(),
        ))
        .unwrap();
        block_on(fixture.access.store.set_turn_usage(
            &turn.id,
            TurnUsage {
                input_tokens: 10,
                output_tokens: 5,
                cost_usd: 0.75,
            },
        ))
        .unwrap();

        assert_eq!(
            fixture.access.attributed_cost(&fixture.origin).unwrap(),
            0.75
        );
        // The fork's own usage is its own: no double counting.
        assert_eq!(fixture.access.attributed_cost(&fork).unwrap(), 0.75);
    }

    #[test]
    fn await_completion_polls_the_registry_and_times_out() {
        let fixture = fixture();
        let host = host(&fixture);

        fixture.completions.publish(
            &fixture.origin,
            Completion {
                stop_reason: "end_turn".to_owned(),
                final_message: "done".to_owned(),
                input_tokens: 1,
                output_tokens: 1,
            },
        );
        let completion = host.await_completion(&fixture.origin, 5).unwrap();
        assert_eq!(completion.stop_reason, "end_turn");
        assert_eq!(completion.final_message, "done");

        // Nothing published: the timeout expires.
        let err = host.await_completion(&fixture.origin, 0).unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
    }

    #[test]
    fn insert_seed_appends_harness_authored_content() {
        let fixture = fixture();
        fixture
            .access
            .insert_seed(&fixture.origin, "Forked for a fresh start.")
            .unwrap();

        let assembled = block_on(fixture.access.store.assemble_context(&fixture.origin)).unwrap();
        let seed = assembled
            .iter()
            .find(|item| item.turn_kind == TurnKind::Seed)
            .unwrap();
        assert!(seed.message.content.contains("Forked for a fresh start."));
    }

    #[test]
    fn record_compaction_clamps_the_retained_boundary() {
        let fixture = fixture();

        // Chain: t1 -> t2 -> c1(first_retained=t1) -> t3 (head).
        let t1 = block_on(fixture.access.store.append_turn(
            &fixture.origin,
            None,
            TurnKind::Interaction,
            None,
            Default::default(),
        ))
        .unwrap()
        .id;
        let t2 = block_on(fixture.access.store.append_turn(
            &fixture.origin,
            Some(&t1),
            TurnKind::Interaction,
            None,
            Default::default(),
        ))
        .unwrap()
        .id;
        let c1 = block_on(fixture.access.store.append_turn(
            &fixture.origin,
            Some(&t2),
            TurnKind::Compaction,
            Some(&t1),
            Default::default(),
        ))
        .unwrap()
        .id;
        let t3 = block_on(fixture.access.store.append_turn(
            &fixture.origin,
            Some(&c1),
            TurnKind::Interaction,
            None,
            Default::default(),
        ))
        .unwrap()
        .id;

        // A compaction whose retained boundary sits at-or-older than
        // c1 clamps to the turn immediately newer than c1 — t3.
        fixture
            .access
            .record_compaction(&fixture.origin, "summary", Some(&t2))
            .unwrap();
        let walk = block_on(fixture.access.store.session_walk(&fixture.origin)).unwrap();
        // The walk from the new head: [c2, t3] — c2's boundary is t3.
        assert_eq!(walk.len(), 2);
        assert_eq!(walk[0].1, TurnKind::Compaction);
        assert_eq!(walk[1].0, t3);
        let c2 = block_on(fixture.access.store.get_turn(&walk[0].0))
            .unwrap()
            .unwrap();
        assert_eq!(c2.first_retained_turn_id.as_deref(), Some(t3.as_str()));

        // A boundary strictly newer than the newest compaction is kept;
        // here the only boundary strictly newer than c2 is none — the
        // compaction elides everything before the summary.
        fixture
            .access
            .record_compaction(&fixture.origin, "second summary", Some(&t3))
            .unwrap();
        let walk = block_on(fixture.access.store.session_walk(&fixture.origin)).unwrap();
        assert_eq!(walk[0].1, TurnKind::Compaction);
        let c3 = block_on(fixture.access.store.get_turn(&walk[0].0))
            .unwrap()
            .unwrap();
        assert_eq!(c3.first_retained_turn_id, None);

        // A foreign turn id is refused.
        let err = fixture
            .access
            .record_compaction(&fixture.origin, "x", Some(&"not-a-turn".to_owned()))
            .unwrap_err();
        assert!(err.to_string().contains("not on the chain"), "{err}");
    }

    #[test]
    fn set_session_title_updates_the_session() {
        let fixture = fixture();
        fixture
            .access
            .set_session_title(&fixture.origin, "Fork lineage")
            .unwrap();
        let session = block_on(fixture.access.store.get_session(&fixture.origin))
            .unwrap()
            .unwrap();
        assert_eq!(session.title, "Fork lineage");
    }

    #[test]
    fn history_search_is_row_and_byte_capped() {
        let fixture = fixture();
        for i in 0..30 {
            fixture
                .access
                .send_prompt(&fixture.origin, &format!("needle {i}"))
                .unwrap();
        }
        let rows = fixture
            .access
            .history_search(&fixture.origin, "needle")
            .unwrap();
        assert_eq!(rows.len(), HISTORY_MAX_ROWS);
        assert!(rows.iter().all(|row| row.content.contains("needle")));

        let rows = fixture
            .access
            .history_search(&fixture.origin, "missing")
            .unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn tokens_are_estimated_at_four_bytes_each() {
        assert_eq!(estimate_tokens(0), 0);
        assert_eq!(estimate_tokens(3), 0);
        assert_eq!(estimate_tokens(4), 1);
        assert_eq!(estimate_tokens(7), 1);
        assert_eq!(estimate_tokens(400), 100);
    }

    #[test]
    fn context_usage_reports_used_and_size() {
        let fixture = fixture();
        fixture
            .access
            .send_prompt(&fixture.origin, "hello context")
            .unwrap();
        let usage = fixture.access.context_usage(&fixture.origin).unwrap();
        assert!(usage.used > 0);
        assert_eq!(usage.size, 100_000);
    }

    #[test]
    fn the_host_api_is_registered_on_a_behaviour_engine() {
        let fixture = fixture();
        let store_host = host(&fixture);

        let compiled = crate::scripts::behaviour_engine(
            r#"
                fn run(request) {
                    let fork = fork_session(#{ ephemeral: true, actor: "summariser" });
                    fork.session_id
                }
            "#,
            &|engine| register_host_api(engine, Arc::clone(&store_host)),
        )
        .unwrap();
        let fork_id = compiled.call("run", &serde_json::json!({})).unwrap();
        let fork_id = fork_id.as_str().unwrap().to_owned();
        let fork = block_on(fixture.access.store.get_session(&fork_id))
            .unwrap()
            .unwrap();
        assert_eq!(fork.kind, SessionKind::Ephemeral);

        // Budgets surface as script errors through the boundary.
        let compiled = crate::scripts::behaviour_engine(
            r#"
                fn run(request) {
                    let fork = fork_session(#{ ephemeral: true });
                    send_prompt(fork.session_id, "one");
                    send_prompt(fork.session_id, "two");
                    send_prompt(fork.session_id, "three");
                }
            "#,
            &|engine| register_host_api(engine, Arc::clone(&store_host)),
        )
        .unwrap();
        let err = compiled.call("run", &serde_json::json!({})).unwrap_err();
        assert!(err.to_string().contains("prompts per script"), "{err}");
    }
