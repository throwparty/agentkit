//! Fork integration tests (T-027): the RFD path, the v1 /fork fallback,
//! seed insertion, fork titles, and script-failure grace.

use agentkit_tackle::agent_client_protocol::schema::v1::ForkSessionRequest;
use agentkit_tackle::agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest, SessionNotification,
    SessionUpdate, TextContent,
};
use agentkit_tackle::agent_client_protocol::schema::ProtocolVersion;
use agentkit_tackle::agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo};
use agentkit_tackle::store::{SessionStore, TurnKind};
use std::sync::{Arc as StdArc, Mutex as StdMutex};

fn spawn_agent(dir: &std::path::Path) -> AcpAgent {
    AcpAgent::from_args([
        env!("CARGO_BIN_EXE_agentkit-tackle"),
        "stdio",
        "--config-dir",
        dir.join("cfg").to_str().unwrap(),
        "--db-path",
        dir.join("sessions.db").to_str().unwrap(),
    ])
    .expect("agent arguments")
}

fn spawn_agent_with_config(dir: &std::path::Path) -> AcpAgent {
    let cfg = dir.join("cfg");
    std::fs::create_dir_all(cfg.join("scripts")).unwrap();
    std::fs::write(
        cfg.join("config.toml"),
        "[scripts.broken-fork]\nevents = [\"session_forked\"]\nfile = \"broken.rhai\"\n",
    )
    .unwrap();
    std::fs::write(
        cfg.join("scripts").join("broken.rhai"),
        "this is not rhai {",
    )
    .unwrap();
    AcpAgent::from_args([
        env!("CARGO_BIN_EXE_agentkit-tackle"),
        "stdio",
        "--config-dir",
        cfg.to_str().unwrap(),
        "--db-path",
        dir.join("sessions.db").to_str().unwrap(),
    ])
    .expect("agent arguments")
}

#[tokio::test(flavor = "multi_thread")]
async fn rfd_fork_creates_a_titled_fork_with_lineage() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("sessions.db");
    let agent = spawn_agent(dir.path());

    Client
        .builder()
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            let created = connection
                .send_request(NewSessionRequest::new(std::path::PathBuf::from("/work")))
                .block_task()
                .await?;
            let source = created.session_id.to_string();

            // The parent carries a title: the fork inherits it,
            // derived from the parent at creation.
            {
                let db = SessionStore::connect_sqlite(&db_path).await.unwrap();
                db.set_title(&source.replace("sess_", ""), "Lineage check")
                    .await
                    .unwrap();
            }

            let forked = connection
                .send_request(ForkSessionRequest::new(
                    created.session_id.clone(),
                    std::path::PathBuf::from("/work"),
                ))
                .block_task()
                .await?;
            let fork_id = forked.session_id.to_string();
            assert_ne!(fork_id, source);

            // The fork is a separate, listable session.
            let listed = connection
                .send_request(
                    agentkit_tackle::agent_client_protocol::schema::v1::ListSessionsRequest::new(),
                )
                .block_task()
                .await?;
            let titles: Vec<(String, Option<String>)> = listed
                .sessions
                .iter()
                .map(|info| {
                    (
                        info.session_id.to_string(),
                        info.title.as_ref().map(|title| title.to_string()),
                    )
                })
                .collect();
            assert_eq!(titles.len(), 2);
            assert!(titles.contains(&(fork_id.clone(), Some("Lineage check".to_owned()))));
            assert!(titles.contains(&(source.clone(), Some("Lineage check".to_owned()))));

            // Lineage in the store: fork point and source parent.
            let db = SessionStore::connect_sqlite(&db_path).await.unwrap();
            let fork_session = db
                .get_session(&fork_id.replace("sess_", ""))
                .await
                .unwrap()
                .expect("fork stored");
            assert_eq!(
                fork_session.forked_from_session_id.as_deref(),
                Some(source.replace("sess_", "").as_str())
            );
            // The fork point is the source's head at creation time.
            let source_session = db
                .get_session(&source.replace("sess_", ""))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(fork_session.fork_point_turn_id, source_session.head_turn_id);
            assert_eq!(
                fork_session.kind,
                agentkit_tackle::store::SessionKind::Interactive
            );
            assert_eq!(fork_session.title, "Lineage check");
            Ok(())
        })
        .await
        .expect("rfd fork round-trip");
}

#[tokio::test(flavor = "multi_thread")]
async fn fallback_fork_reports_the_id_and_seeds() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("sessions.db");
    let agent = spawn_agent(dir.path());

    let seen: StdArc<StdMutex<Vec<String>>> = StdArc::default();
    let seen_handler = seen.clone();

    Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                if let SessionUpdate::AgentMessageChunk(chunk) = &notification.update {
                    if let ContentBlock::Text(text) = &chunk.content {
                        seen_handler.lock().unwrap().push(text.text.clone());
                    }
                }
                Ok(())
            },
            agentkit_tackle::agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let created = connection
                .send_request(NewSessionRequest::new(std::path::PathBuf::from("/work")))
                .block_task()
                .await?;
            let source = created.session_id.to_string();

            // The /fork fallback: no model request — the response is
            // end_turn and the new session id is reported in the
            // parent turn.
            let response = connection
                .send_request(PromptRequest::new(
                    created.session_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("/fork".to_owned()))],
                ))
                .block_task()
                .await?;
            assert!(matches!(
                response.stop_reason,
                agentkit_tackle::agent_client_protocol::schema::v1::StopReason::EndTurn
            ));

            // The report surfaced as an agent chunk carrying the fork's
            // wire id.
            let chunks = seen.lock().unwrap().clone();
            let report = chunks
                .iter()
                .find(|text| text.contains("sess_"))
                .expect("fork report chunk");
            assert!(report.contains("session"), "{report}");

            // The fork exists with the harness-authored seed; the
            // parent turn holds the report as an agent message.
            let db = SessionStore::connect_sqlite(&db_path).await.unwrap();
            let listed = db
                .list_sessions(&agentkit_tackle::store::ListFilter::default())
                .await
                .unwrap();
            assert_eq!(listed.len(), 2);
            let fork_session = listed
                .iter()
                .find(|session| session.id != source.replace("sess_", ""))
                .unwrap();

            let assembled = db.assemble_context(&fork_session.id).await.unwrap();
            let seed = assembled
                .iter()
                .find(|item| item.turn_kind == TurnKind::Seed)
                .expect("seed inserted");
            assert!(seed.message.content.contains("Forked from session"));
            assert_eq!(seed.message.role, agentkit_tackle::store::Role::User);

            let parent_assembled = db
                .assemble_context(&source.replace("sess_", ""))
                .await
                .unwrap();
            let report_stored = parent_assembled.iter().any(|item| {
                item.message.role == agentkit_tackle::store::Role::Assistant
                    && item.message.content.contains("sess_")
            });
            assert!(report_stored, "the report is in the parent turn");
            Ok(())
        })
        .await
        .expect("fallback fork round-trip");
}

#[tokio::test(flavor = "multi_thread")]
async fn script_failure_never_fails_the_fork() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("sessions.db");
    let agent = spawn_agent_with_config(dir.path());

    Client
        .builder()
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let created = connection
                .send_request(NewSessionRequest::new(std::path::PathBuf::from("/work")))
                .block_task()
                .await?;

            // The registered session_forked script does not compile —
            // the fork still succeeds.
            let forked = connection
                .send_request(ForkSessionRequest::new(
                    created.session_id.clone(),
                    std::path::PathBuf::from("/work"),
                ))
                .block_task()
                .await?;
            let db = SessionStore::connect_sqlite(&db_path).await.unwrap();
            let fork_session = db
                .get_session(&forked.session_id.to_string().replace("sess_", ""))
                .await
                .unwrap()
                .expect("the fork exists despite the script failure");
            assert_eq!(fork_session.cwd, "/work");
            Ok(())
        })
        .await
        .expect("fork survives a failing script");
}
