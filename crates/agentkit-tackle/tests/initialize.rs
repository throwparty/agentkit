//! Initialize-phase integration tests: the golden transcript harness for
//! the conformance suite (T-013) starts here. Each test spawns the tackle
//! binary as a subprocess and drives it with the SDK's client — the same
//! shape the golden transcripts replay.

use agentkit_tackle::agent_client_protocol::schema::v1::{InitializeRequest, SessionCapabilities};
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

            Ok(())
        })
        .await
        .expect("initialize handshake");
}
