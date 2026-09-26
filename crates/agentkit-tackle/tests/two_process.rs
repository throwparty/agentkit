//! Two-process integration tests (T-033): two tackle processes sharing
//! one session database — cross-process session/list visibility, lease
//! contention with precise errors, and fork lineage across processes.

use agentkit_tackle::agent_client_protocol::schema::v1::ForkSessionRequest;
use agentkit_tackle::agent_client_protocol::schema::v1::{
    InitializeRequest, ListSessionsRequest, NewSessionRequest, PromptRequest, TextContent,
};
use agentkit_tackle::agent_client_protocol::schema::ProtocolVersion;
use agentkit_tackle::agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo};
use agentkit_tackle::store::SessionStore;

fn spawn_agent(dir: &std::path::Path, name: &str) -> AcpAgent {
    AcpAgent::from_args([
        env!("CARGO_BIN_EXE_agentkit-tackle"),
        "stdio",
        "--config-dir",
        dir.join(name).to_str().unwrap(),
        "--db-path",
        dir.join("sessions.db").to_str().unwrap(),
    ])
    .expect("agent arguments")
}

/// A slow /compact script: the behaviour engine's busy loop holds the
/// owning process's lease for the duration.
fn spawn_agent_with_slow_compaction(dir: &std::path::Path, name: &str) -> AcpAgent {
    let cfg = dir.join(name);
    std::fs::create_dir_all(cfg.join("scripts")).unwrap();
    std::fs::create_dir_all(cfg.join("prompts")).unwrap();
    std::fs::write(
        cfg.join("config.toml"),
        "[scripts.slow-compact]\nevents = [\"compaction_requested\"]\nfile = \"slow.rhai\"\n",
    )
    .unwrap();
    std::fs::write(
        cfg.join("scripts").join("slow.rhai"),
        "fn compaction_requested(event) { let i = 0; while i < 200_000_000 { i = i + 1; } }",
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
async fn sessions_are_visible_across_processes() {
    let dir = tempfile::tempdir().unwrap();
    let agent_a = spawn_agent(dir.path(), "a");
    let agent_b = spawn_agent(dir.path(), "b");

    // Process A creates a session; process B lists it.
    Client
        .builder()
        .connect_with(agent_a, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            connection
                .send_request(NewSessionRequest::new(std::path::PathBuf::from(
                    "/shared-work",
                )))
                .block_task()
                .await?;
            Ok(())
        })
        .await
        .expect("process A creates");

    Client
        .builder()
        .connect_with(agent_b, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let listed = connection
                .send_request(ListSessionsRequest::new())
                .block_task()
                .await?;
            let cwds: Vec<String> = listed
                .sessions
                .iter()
                .map(|info| info.cwd.to_string_lossy().to_string())
                .collect();
            assert!(
                cwds.iter().any(|cwd| cwd == "/shared-work"),
                "process B sees process A's session: {cwds:?}"
            );
            Ok(())
        })
        .await
        .expect("process B observes");
}

#[tokio::test(flavor = "multi_thread")]
async fn lease_contention_fails_precisely() {
    let dir = tempfile::tempdir().unwrap();
    // Separate processes: A creates the session, A' runs the slow
    // /compact holding the lease, B hits the contention error.
    let agent_a1 = spawn_agent(dir.path(), "a1");
    let agent_a2 = spawn_agent_with_slow_compaction(dir.path(), "a2");
    let agent_b = spawn_agent(dir.path(), "b");

    // Process A creates the session and runs the slow /compact,
    // holding the lease; process B's prompt hits the contention error.
    let created_id = Client
        .builder()
        .connect_with(agent_a1, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let created = connection
                .send_request(NewSessionRequest::new(std::path::PathBuf::from(
                    "/contention",
                )))
                .block_task()
                .await?;
            Ok(created.session_id.to_string())
        })
        .await
        .expect("process A creates");

    let (a_prompt, b_prompt) = tokio::join!(
        async {
            let created_id = created_id.clone();
            Client
                .builder()
                .connect_with(agent_a2, |connection: ConnectionTo<Agent>| async move {
                    connection
                        .send_request(InitializeRequest::new(ProtocolVersion::V1))
                        .block_task()
                        .await?;
                    connection
                        .send_request(PromptRequest::new(
                            agentkit_tackle::agent_client_protocol::schema::v1::SessionId::new(
                                created_id.clone(),
                            ),
                            vec![agentkit_tackle::agent_client_protocol::schema::v1::ContentBlock::Text(
                                TextContent::new("/compact".to_owned()),
                            )],
                        ))
                        .block_task()
                        .await
                })
                .await
        },
        async {
            // A short delay so A's lease lands first.
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let created_id = created_id.clone();
            Client
                .builder()
                .connect_with(agent_b, |connection: ConnectionTo<Agent>| async move {
                    connection
                        .send_request(InitializeRequest::new(ProtocolVersion::V1))
                        .block_task()
                        .await?;
                    connection
                        .send_request(PromptRequest::new(
                            agentkit_tackle::agent_client_protocol::schema::v1::SessionId::new(
                                created_id.clone(),
                            ),
                            vec![agentkit_tackle::agent_client_protocol::schema::v1::ContentBlock::Text(
                                TextContent::new("any prompt".to_owned()),
                            )],
                        ))
                        .block_task()
                        .await
                })
                .await
        }
    );

    // A's /compact completes (the slow script runs to its end).
    a_prompt.expect("process A's compaction completes");

    // B's prompt failed precisely: session-actively-owned with the
    // owning connection named.
    let err = b_prompt.expect_err("lease contention is an error");
    let data = err.data.expect("error data carries the reason").to_string();
    assert!(data.contains("session-actively-owned"), "{data}");
}

#[tokio::test(flavor = "multi_thread")]
async fn fork_lineage_spans_processes() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("sessions.db");
    let agent_a = spawn_agent(dir.path(), "a");
    let agent_b = spawn_agent(dir.path(), "b");

    let source_id = Client
        .builder()
        .connect_with(agent_a, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let created = connection
                .send_request(NewSessionRequest::new(std::path::PathBuf::from("/lineage")))
                .block_task()
                .await?;
            Ok(created.session_id.to_string())
        })
        .await
        .expect("process A creates");

    // Process B forks process A's session.
    let fork_source = source_id.clone();
    let fork_id = Client
        .builder()
        .connect_with(agent_b, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let forked = connection
                .send_request(ForkSessionRequest::new(
                    agentkit_tackle::agent_client_protocol::schema::v1::SessionId::new(
                        fork_source.clone(),
                    ),
                    std::path::PathBuf::from("/lineage"),
                ))
                .block_task()
                .await?;
            Ok(forked.session_id.to_string())
        })
        .await
        .expect("process B forks");

    // The lineage in the shared store: the fork names process A's
    // session as its source.
    let db = SessionStore::connect_sqlite(&db_path).await.unwrap();
    let fork_session = db
        .get_session(&fork_id.replace("sess_", ""))
        .await
        .unwrap()
        .expect("the fork is in the shared store");
    assert_eq!(
        fork_session.forked_from_session_id.as_deref(),
        Some(source_id.replace("sess_", "").as_str())
    );
}
