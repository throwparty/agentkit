//! Turn cancellation over the in-process client harness: `session/cancel`
//! resolves an in-flight `session/prompt` with `stopReason: cancelled`.

use agentkit_tackle::agent_client_protocol::schema::v1::{
    CancelNotification, CloseSessionRequest, ContentBlock, InitializeRequest, ListSessionsRequest,
    NewSessionRequest, PromptRequest, StopReason, TextContent,
};
use agentkit_tackle::agent_client_protocol::schema::ProtocolVersion;
use agentkit_tackle::agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Every model request is held open far longer than the test needs, so the
/// turn is still in flight when the cancel lands.
struct HangingModel;

impl Respond for HangingModel {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(120))
    }
}

fn write_config(dir: &std::path::Path, endpoint_uri: &str) {
    let cfg = dir.join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("config.toml"),
        format!(
            r#"[endpoints.mock]
base_url = "{endpoint_uri}"
wire_format = "openai-chat-completions"
auth = "none"
models = ["gpt-4o-mini"]

[defaults]
model = "mock/gpt-4o-mini"
"#
        ),
    )
    .unwrap();
}

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

#[tokio::test(flavor = "multi_thread")]
async fn session_cancel_resolves_the_prompt_as_cancelled() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(HangingModel)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), &server.uri());
    let agent = spawn_agent(dir.path());

    Client
        .builder()
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let session = connection
                .send_request(NewSessionRequest::new(std::path::PathBuf::from("/work")))
                .block_task()
                .await?;
            let session_id = session.session_id.clone();

            let prompt = connection.send_request(PromptRequest::new(
                session_id.clone(),
                vec![ContentBlock::Text(TextContent::new("hello".to_owned()))],
            ));
            // A notification, so it reaches the dispatch loop after the
            // prompt handler that registered the turn's token.
            connection.send_notification(CancelNotification::new(session_id.clone()))?;
            let response = prompt.block_task().await?;
            assert_eq!(response.stop_reason, StopReason::Cancelled);
            Ok(())
        })
        .await
        .expect("cancelled turn");
}

#[tokio::test(flavor = "multi_thread")]
async fn session_cancel_for_an_idle_session_is_a_noop() {
    let dir = tempfile::tempdir().unwrap();
    // No model request is made, so the endpoint is never contacted.
    write_config(dir.path(), "http://127.0.0.1:1/v1");
    let agent = spawn_agent(dir.path());

    Client
        .builder()
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let session = connection
                .send_request(NewSessionRequest::new(std::path::PathBuf::from("/work")))
                .block_task()
                .await?;
            connection.send_notification(CancelNotification::new(session.session_id.clone()))?;
            // The connection is unaffected: the next request round-trips.
            connection
                .send_request(ListSessionsRequest::new())
                .block_task()
                .await?;
            Ok(())
        })
        .await
        .expect("idle cancel is a no-op");
}

#[tokio::test(flavor = "multi_thread")]
async fn session_close_cancels_the_in_flight_prompt() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(HangingModel)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), &server.uri());
    let agent = spawn_agent(dir.path());

    Client
        .builder()
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let session = connection
                .send_request(NewSessionRequest::new(std::path::PathBuf::from("/work")))
                .block_task()
                .await?;
            let session_id = session.session_id.clone();

            let prompt = connection.send_request(PromptRequest::new(
                session_id.clone(),
                vec![ContentBlock::Text(TextContent::new("hello".to_owned()))],
            ));
            let close = connection.send_request(CloseSessionRequest::new(session_id.clone()));

            let response = prompt.block_task().await?;
            assert_eq!(response.stop_reason, StopReason::Cancelled);
            close.block_task().await?;

            let listed = connection
                .send_request(ListSessionsRequest::new())
                .block_task()
                .await?;
            assert!(
                listed
                    .sessions
                    .iter()
                    .any(|session| session.session_id == session_id),
                "close keeps the session listable"
            );
            Ok(())
        })
        .await
        .expect("close during a turn");
}
