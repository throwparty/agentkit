//! Store tests: backend parity, migrations, leases, soft delete, and the daggy assembly oracle.

use super::session::Backend;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use super::*;
use std::collections::BTreeMap;

    #[tokio::test]
    async fn sqlite_connects_migrates_and_restricts_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        let store = SessionStore::connect_sqlite(&path).await.unwrap();

        // Migration ran: sessions table is queryable.
        let sessions = store.list_sessions(&ListFilter::default()).await.unwrap();
        assert!(sessions.is_empty());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[tokio::test]
    async fn dead_schema_is_not_created() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::connect_sqlite(&dir.path().join("sessions.db"))
            .await
            .unwrap();
        let Backend::Sqlite(pool) = &store.backend else {
            panic!("expected sqlite backend");
        };

        let tables: Vec<String> =
            sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table'")
                .fetch_all(pool)
                .await
                .unwrap();
        assert!(
            !tables
                .iter()
                .any(|table| table == "definitions" || table == "turn_definitions"),
            "{tables:?}"
        );

        let turn_columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('turns')")
                .fetch_all(pool)
                .await
                .unwrap();
        let message_columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('messages')")
                .fetch_all(pool)
                .await
                .unwrap();
        assert!(
            !turn_columns.iter().any(|column| column == "metadata"),
            "turns.metadata still present: {turn_columns:?}"
        );
        assert!(
            !message_columns.iter().any(|column| column == "metadata"),
            "messages.metadata still present: {message_columns:?}"
        );
    }

    #[tokio::test]
    async fn sqlite_connects_through_missing_leading_directories() {
        let dir = tempfile::tempdir().unwrap();
        // The data dir's full chain may not exist yet — the store
        // creates it rather than failing to open.
        let path = dir.path().join("state/agentkit/tackle/sessions.db");
        assert!(!path.parent().unwrap().exists());
        SessionStore::connect_sqlite(&path).await.unwrap();
        assert!(path.is_file());
    }

    #[tokio::test]
    async fn session_crud_round_trips_on_memory() {
        let store = SessionStore::in_memory();
        crud_round_trip(&store).await.unwrap();
    }

    #[tokio::test]
    async fn session_crud_round_trips_on_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::connect_sqlite(&dir.path().join("sessions.db"))
            .await
            .unwrap();
        crud_round_trip(&store).await.unwrap();
    }

    async fn crud_round_trip(store: &SessionStore) -> Result<(), StoreError> {
        let session = store
            .create_session(SessionKind::Interactive, "/work", None, None, "{}")
            .await?;
        assert!(session.head_turn_id.is_none());

        // Update-list requires a later updated_at for ordering; both created
        // in the same second, so ordering assertions only cover membership.
        let listed = store.list_sessions(&ListFilter::default()).await?;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, session.id);

        store.close_session(&session.id).await?;
        let listed = store.list_sessions(&ListFilter::default()).await?;
        assert_eq!(listed.len(), 1, "close keeps the session listable");

        store.delete_session(&session.id).await?;
        let listed = store.list_sessions(&ListFilter::default()).await?;
        assert!(listed.is_empty(), "delete hides the session");

        let deleted = store.get_session(&session.id).await?;
        assert!(deleted.is_some_and(|s| !s.active));
        Ok(())
    }

    #[tokio::test]
    async fn turns_messages_and_usage_round_trip() {
        let store = SessionStore::in_memory();
        let session = store
            .create_session(SessionKind::Interactive, "/work", None, None, "{}")
            .await
            .unwrap();

        let turn = store
            .append_turn(
                &session.id,
                None,
                TurnKind::Interaction,
                None,
                TurnUsage {
                    input_tokens: 100,
                    output_tokens: 40,
                    cost_usd: 0.01,
                },
            )
            .await
            .unwrap();
        store
            .append_message(
                &turn.id,
                Role::User,
                "[{\"type\":\"text\"}]",
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        store
            .append_message(&turn.id, Role::Assistant, "[]", None, None, None, None)
            .await
            .unwrap();

        let usage = store.session_usage(&session.id).await.unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 40);
    }

    #[tokio::test]
    async fn ephemeral_sessions_are_filtered_from_listings() {
        let store = SessionStore::in_memory();
        store
            .create_session(SessionKind::Ephemeral, "/work", None, None, "{}")
            .await
            .unwrap();

        let listed = store.list_sessions(&ListFilter::default()).await.unwrap();
        assert!(listed.is_empty());

        let listed = store
            .list_sessions(&ListFilter {
                include_ephemeral: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
    }

    // --- Context assembly ---

    /// Appends one interaction turn with a single user message whose
    /// content identifies the turn.
    async fn append_named_turn(
        store: &SessionStore,
        session_id: &SessionId,
        parent: Option<&TurnId>,
        kind: TurnKind,
        first_retained: Option<&TurnId>,
        content: &str,
    ) -> Turn {
        let turn = store
            .append_turn(
                session_id,
                parent,
                kind,
                first_retained,
                TurnUsage::default(),
            )
            .await
            .unwrap();
        store
            .append_message(
                &turn.id,
                Role::User,
                &format!("[{{\"type\":\"text\",\"text\":\"{content}\"}}]"),
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        turn
    }

    fn assembled_texts(messages: &[AssembledMessage]) -> Vec<String> {
        messages
            .iter()
            .map(|assembled| {
                let blocks: serde_json::Value =
                    serde_json::from_str(&assembled.message.content).unwrap();
                blocks[0]["text"].as_str().expect("text block").to_owned()
            })
            .collect()
    }

    /// An in-memory SQLite store: the fast backend for property tests.
    /// The pool is pinned to one connection — each pooled connection would
    /// otherwise get its own empty `:memory:` database.
    async fn sqlite_memory_store() -> SessionStore {
        let options = SqliteConnectOptions::new()
            .in_memory(true)
            .busy_timeout(std::time::Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        SessionStore {
            backend: Backend::Sqlite(pool),
        }
    }

    #[tokio::test]
    async fn linear_chain_assembles_oldest_first() {
        for store in [SessionStore::in_memory(), sqlite_memory_store().await] {
            let session = store
                .create_session(SessionKind::Interactive, "/work", None, None, "{}")
                .await
                .unwrap();
            let mut parent = None;
            for name in ["t1", "t2", "t3"] {
                let turn = append_named_turn(
                    &store,
                    &session.id,
                    parent.as_ref(),
                    TurnKind::Interaction,
                    None,
                    name,
                )
                .await;
                parent = Some(turn.id);
            }
            let assembled = store.assemble_context(&session.id).await.unwrap();
            assert_eq!(assembled_texts(&assembled), ["t1", "t2", "t3"]);
        }
    }

    #[tokio::test]
    async fn full_compaction_elides_everything_below_the_summary() {
        for store in [SessionStore::in_memory(), sqlite_memory_store().await] {
            let session = store
                .create_session(SessionKind::Interactive, "/work", None, None, "{}")
                .await
                .unwrap();
            let t1 =
                append_named_turn(&store, &session.id, None, TurnKind::Interaction, None, "t1")
                    .await;
            let t2 = append_named_turn(
                &store,
                &session.id,
                Some(&t1.id),
                TurnKind::Interaction,
                None,
                "t2",
            )
            .await;
            append_named_turn(
                &store,
                &session.id,
                Some(&t2.id),
                TurnKind::Compaction,
                None,
                "summary",
            )
            .await;

            let assembled = store.assemble_context(&session.id).await.unwrap();
            assert_eq!(
                assembled_texts(&assembled),
                ["summary"],
                "full compaction keeps only the summary"
            );
        }
    }

    #[tokio::test]
    async fn keep_recent_compaction_retains_range_and_summary_sorts_first() {
        for store in [SessionStore::in_memory(), sqlite_memory_store().await] {
            let session = store
                .create_session(SessionKind::Interactive, "/work", None, None, "{}")
                .await
                .unwrap();
            let t1 =
                append_named_turn(&store, &session.id, None, TurnKind::Interaction, None, "t1")
                    .await;
            let t2 = append_named_turn(
                &store,
                &session.id,
                Some(&t1.id),
                TurnKind::Interaction,
                None,
                "t2",
            )
            .await;
            let t3 = append_named_turn(
                &store,
                &session.id,
                Some(&t2.id),
                TurnKind::Interaction,
                None,
                "t3",
            )
            .await;
            let t4 = append_named_turn(
                &store,
                &session.id,
                Some(&t3.id),
                TurnKind::Interaction,
                None,
                "t4",
            )
            .await;
            // Compaction at t5 keeps t3 onward verbatim; t1..t2 are elided.
            append_named_turn(
                &store,
                &session.id,
                Some(&t4.id),
                TurnKind::Compaction,
                Some(&t3.id),
                "summary",
            )
            .await;

            let assembled = store.assemble_context(&session.id).await.unwrap();
            assert_eq!(
                assembled_texts(&assembled),
                ["summary", "t3", "t4"],
                "summary first, then retained turns oldest to newest"
            );
        }
    }

    #[tokio::test]
    async fn turns_after_compaction_come_after_the_retained_range() {
        for store in [SessionStore::in_memory(), sqlite_memory_store().await] {
            let session = store
                .create_session(SessionKind::Interactive, "/work", None, None, "{}")
                .await
                .unwrap();
            let t1 =
                append_named_turn(&store, &session.id, None, TurnKind::Interaction, None, "t1")
                    .await;
            let c = append_named_turn(
                &store,
                &session.id,
                Some(&t1.id),
                TurnKind::Compaction,
                None,
                "summary",
            )
            .await;
            append_named_turn(
                &store,
                &session.id,
                Some(&c.id),
                TurnKind::Interaction,
                None,
                "t2",
            )
            .await;

            let assembled = store.assemble_context(&session.id).await.unwrap();
            assert_eq!(assembled_texts(&assembled), ["summary", "t2"]);
        }
    }

    /// Seeded xorshift: deterministic property-test generation without a
    /// random-number dependency.
    struct Rng(u64);

    impl Rng {
        fn new(seed: u64) -> Self {
            Self(seed | 1)
        }

        fn below(&mut self, n: u64) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x % n
        }
    }

    #[tokio::test]
    async fn property_sql_matches_memory_and_daggy() {
        for seed in 0..48u64 {
            let mut rng = Rng::new(seed);
            let memory_store = SessionStore::in_memory();
            let sqlite_store = sqlite_memory_store().await;

            let session = memory_store
                .create_session(SessionKind::Interactive, "/work", None, None, "{}")
                .await
                .unwrap();
            let sqlite_session = sqlite_store
                .create_session(SessionKind::Interactive, "/work", None, None, "{}")
                .await
                .unwrap();

            // All turns ever created (the SessionGraph oracle spans the DAG).
            // Backends generate independent uuids; correspondence between
            // memory and sqlite turns is positional, so parent/retained
            // links are chosen as INDICES and mapped per backend.
            let mut all_turns: Vec<Turn> = Vec::new();
            let mut all_turns_sqlite: Vec<Turn> = Vec::new();
            let mut content_by_turn: BTreeMap<TurnId, String> = BTreeMap::new();
            let mut head_idx: Option<usize> = None;
            let mut newest_compaction_idx: Option<usize> = None;

            let count = 8 + rng.below(8);
            for i in 0..count {
                // Fork: occasionally continue from an older turn instead of
                // the head.
                let parent_idx: Option<usize> = if head_idx.is_some() && rng.below(4) == 0 {
                    Some(rng.below(all_turns.len() as u64) as usize)
                } else {
                    head_idx
                };

                let is_compaction = rng.below(5) == 0;
                let (kind, retained_idx) = if is_compaction {
                    // Clamp rule: first_retained must be newer than the
                    // newest existing compaction (or a full compaction).
                    let retained = match newest_compaction_idx {
                        None => {
                            if all_turns.is_empty() {
                                None
                            } else {
                                Some(rng.below(all_turns.len() as u64) as usize)
                            }
                        }
                        Some(newest) => {
                            let newer_span = all_turns.len() - newest - 1;
                            if newer_span > 0 {
                                Some(newest + 1 + rng.below(newer_span as u64) as usize)
                            } else {
                                None
                            }
                        }
                    };
                    (TurnKind::Compaction, retained)
                } else {
                    (TurnKind::Interaction, None)
                };

                let content = format!("s{seed}-t{i}");
                let memory_parent = parent_idx.map(|idx| all_turns[idx].id.clone());
                let sqlite_parent = parent_idx.map(|idx| all_turns_sqlite[idx].id.clone());
                let memory_retained = retained_idx.map(|idx| all_turns[idx].id.clone());
                let sqlite_retained = retained_idx.map(|idx| all_turns_sqlite[idx].id.clone());

                let memory_turn = append_named_turn(
                    &memory_store,
                    &session.id,
                    memory_parent.as_ref(),
                    kind,
                    memory_retained.as_ref(),
                    &content,
                )
                .await;
                // Each backend generates its own uuid; the correspondence
                // is positional, verified via the shared content below.
                let sqlite_turn = append_named_turn(
                    &sqlite_store,
                    &sqlite_session.id,
                    sqlite_parent.as_ref(),
                    kind,
                    sqlite_retained.as_ref(),
                    &content,
                )
                .await;

                content_by_turn.insert(memory_turn.id.clone(), content);
                if kind == TurnKind::Compaction {
                    newest_compaction_idx = Some(all_turns.len());
                }
                head_idx = Some(all_turns.len());
                all_turns.push(memory_turn);
                all_turns_sqlite.push(sqlite_turn);
            }

            // Oracle: daggy graph over all turns, walked from the head.
            let graph = SessionGraph::build(all_turns.iter().cloned());
            let head_id = all_turns[head_idx.unwrap()].id.clone();
            let expected_ids = graph.assembly_order(&head_id);
            let expected_contents: Vec<String> = expected_ids
                .iter()
                .map(|id| content_by_turn[id].clone())
                .collect();

            let memory_assembled = memory_store.assemble_context(&session.id).await.unwrap();
            let sqlite_assembled = sqlite_store
                .assemble_context(&sqlite_session.id)
                .await
                .unwrap();

            assert_eq!(
                assembled_texts(&memory_assembled),
                expected_contents,
                "seed {seed}: memory assembly must match the daggy oracle"
            );
            assert_eq!(
                assembled_texts(&sqlite_assembled),
                expected_contents,
                "seed {seed}: SQL assembly must match the daggy oracle"
            );
        }
    }

    // --- Ownership leases ---

    async fn lease_round_trip(store: &SessionStore) -> Result<(), StoreError> {
        let session = store
            .create_session(SessionKind::Interactive, "/work", None, None, "{}")
            .await?;

        // Contention: the first owner wins; the second is refused.
        assert!(store.acquire_lease(&session.id, "conn-a", 60).await?);
        assert!(!store.acquire_lease(&session.id, "conn-b", 60).await?);
        assert_eq!(
            store.lease_holder(&session.id).await?,
            Some("conn-a".into())
        );

        // Heartbeat by the owner refreshes; by another is refused.
        assert!(store.heartbeat_lease(&session.id, "conn-a", 60).await?);
        assert!(!store.heartbeat_lease(&session.id, "conn-b", 60).await?);

        // Soft delete is refused while actively leased.
        let err = store.delete_session(&session.id).await.unwrap_err();
        assert!(
            matches!(&err, StoreError::LeaseHeld { owner, .. } if owner == "conn-a"),
            "{err}"
        );

        // Close releases the lease; delete then succeeds.
        store.close_session(&session.id).await?;
        assert_eq!(store.lease_holder(&session.id).await?, None);
        store.delete_session(&session.id).await?;
        Ok(())
    }

    #[tokio::test]
    async fn lease_round_trips_on_memory() {
        lease_round_trip(&SessionStore::in_memory()).await.unwrap();
    }

    #[tokio::test]
    async fn lease_round_trips_on_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::connect_sqlite(&dir.path().join("sessions.db"))
            .await
            .unwrap();
        lease_round_trip(&store).await.unwrap();
    }

    #[tokio::test]
    async fn expired_leases_are_stealable() -> Result<(), StoreError> {
        for store in [SessionStore::in_memory(), sqlite_memory_store().await] {
            let session = store
                .create_session(SessionKind::Interactive, "/work", None, None, "{}")
                .await?;
            // A lease already expired when acquired: a negative ttl puts
            // the expiry in the past.
            assert!(store.acquire_lease(&session.id, "stale", -1).await?);
            assert!(store.acquire_lease(&session.id, "fresh", 60).await?);
            assert_eq!(store.lease_holder(&session.id).await?, Some("fresh".into()));
        }
        Ok(())
    }
