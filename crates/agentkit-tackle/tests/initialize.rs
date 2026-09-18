//! Initialize-phase integration tests: the golden transcript harness for
//! the conformance suite (T-013) starts here. Each test spawns the tackle
//! binary as a subprocess and drives it with the SDK's client — the same
//! shape the golden transcripts replay.

use agentkit_tackle::agent_client_protocol::schema::v1::{
    InitializeRequest, ListSessionsRequest, NewSessionRequest, SessionCapabilities,
};
use agentkit_tackle::agent_client_protocol::schema::ProtocolVersion;
use agentkit_tackle::agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo};

/// Spawns tackle as a subprocess against an isolated configuration.
fn spawn_agent(dir: &std::path::Path) -> AcpAgent {
    AcpAgent::from_args([
        env!("CARGO_BIN_EXE_agentkit-tackle"),
        "--config-dir",
        dir.join("cfg").to_str().unwrap(),
        "--db-path",
        dir.join("sessions.db").to_str().unwrap(),
    ])
    .expect("agent arguments")
}

#[tokio::test(flavor = "multi_thread")]
async fn initialize_negotiates_v1_and_advertises_capabilities() {
    let dir = tempfile::tempdir().unwrap();
    let agent = spawn_agent(dir.path());

    Client
        .builder()
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            let response = connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            // Negotiation: the client asked for v1; the agent supports v1,
            // so it responds with the same version.
            assert_eq!(response.protocol_version, ProtocolVersion::V1);

            // Capability advertisement per FR-002: the stable surface.
            let capabilities = &response.agent_capabilities;
            assert!(capabilities.load_session, "loadSession must be advertised");
            let session_capabilities: &SessionCapabilities = &capabilities.session_capabilities;
            assert!(session_capabilities.list.is_some(), "session/list");
            assert!(session_capabilities.close.is_some(), "session/close");
            assert!(session_capabilities.resume.is_some(), "session/resume");
            assert!(session_capabilities.delete.is_some(), "session/delete");
            assert!(capabilities.prompt_capabilities.image, "image prompts");
            assert!(!capabilities.prompt_capabilities.audio, "no audio in v1");
            assert!(capabilities.mcp_capabilities.http, "MCP over HTTP");
            // No authentication in v1.
            assert!(response.auth_methods.is_empty());

            // Unstable features are advertised via _meta and the fork
            // capability (gated on SENDING, not advertising — FR-002).
            let meta = response
                .agent_capabilities
                .meta
                .as_ref()
                .expect("unstable _meta");
            assert!(meta.contains_key("unstable_session_fork"));
            assert!(meta.contains_key("unstable_session_compaction"));
            assert!(meta.contains_key("unstable_session_notices"));
            assert!(
                session_capabilities.fork.is_some(),
                "session/fork capability"
            );

            Ok(())
        })
        .await
        .expect("initialize handshake");
}

#[tokio::test(flavor = "multi_thread")]
async fn session_lifecycle_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let agent = spawn_agent(dir.path());

    Client
        .builder()
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            // session/new: immediate return with a session id.
            let created = connection
                .send_request(NewSessionRequest::new(std::path::PathBuf::from("/work")))
                .block_task()
                .await?;
            let session_id = created.session_id.clone();

            // session/list: the new session appears, ordered, with cwd.
            let listed = connection
                .send_request(ListSessionsRequest::new())
                .block_task()
                .await?;
            assert_eq!(listed.sessions.len(), 1);
            assert_eq!(listed.sessions[0].session_id, session_id);
            assert_eq!(listed.sessions[0].cwd, std::path::PathBuf::from("/work"));
            assert!(listed.next_cursor.is_none());

            // session/close: the session stays listable.
            let closed = connection
                .send_request(
                    agentkit_tackle::agent_client_protocol::schema::v1::CloseSessionRequest::new(
                        session_id.clone(),
                    ),
                )
                .block_task()
                .await?;
            let _ = closed;
            let listed = connection
                .send_request(ListSessionsRequest::new())
                .block_task()
                .await?;
            assert_eq!(listed.sessions.len(), 1, "close keeps the session listable");

            // session/delete: hides it.
            let deleted = connection
                .send_request(
                    agentkit_tackle::agent_client_protocol::schema::v1::DeleteSessionRequest::new(
                        session_id.clone(),
                    ),
                )
                .block_task()
                .await?;
            let _ = deleted;
            let listed = connection
                .send_request(ListSessionsRequest::new())
                .block_task()
                .await?;
            assert!(listed.sessions.is_empty(), "delete hides the session");

            Ok(())
        })
        .await
        .expect("session lifecycle round-trip");
}
