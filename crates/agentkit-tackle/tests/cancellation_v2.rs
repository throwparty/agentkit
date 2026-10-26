//! v2 turn cancellation: `session/cancel`, `session/close`, and a cancel
//! after completion, observed through the v2 idle state machine.

use agentkit_tackle::agent_client_protocol::schema::v2;
use agentkit_tackle::agent_client_protocol::schema::ProtocolVersion;
use agentkit_tackle::agent_client_protocol::{
    AcpAgent, Agent, Client, V2ConnectionTo, on_receive_notification,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Every model request is held open far longer than the test needs, so the
/// turn is still in flight when the cancel lands.
struct HangingModel;

impl Respond for HangingModel {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        ResponseTemplate::new(200).set_delay(Duration::from_secs(120))
    }
}

/// Hangs only on the `HANG` prompt, so a later prompt in the same session is
/// answered and the lease release is observable.
struct SentinelledModel;

impl Respond for SentinelledModel {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let tail = body["messages"]
            .as_array()
            .and_then(|messages| messages.last())
            .map(|message| &message["content"]);
        let last_part = match tail {
            Some(serde_json::Value::String(text)) => text.clone(),
            Some(serde_json::Value::Array(parts)) => parts
                .last()
                .and_then(|part| part["text"].as_str())
                .unwrap_or_default()
                .to_owned(),
            _ => String::new(),
        };
        if last_part == "HANG" {
            return ResponseTemplate::new(200).set_delay(Duration::from_secs(120));
        }
        let chunk = |delta: serde_json::Value, finish: Option<&str>| {
            serde_json::json!({
                "id": "c1",
                "object": "chat.completion.chunk",
                "created": 0,
                "model": "m",
                "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
            })
            .to_string()
        };
        let lines = [
            format!(
                "data: {}",
                chunk(serde_json::json!({ "role": "assistant" }), None)
            ),
            format!(
                "data: {}",
                chunk(serde_json::json!({ "content": "ok" }), None)
            ),
            format!("data: {}", chunk(serde_json::json!({}), Some("stop"))),
            String::new(),
        ];
        ResponseTemplate::new(200).set_body_raw(lines.join("\n\n") + "\n", "text/event-stream")
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

[scripts.titling]
events = ["title_trigger"]
file = "builtin:titling.rhai"
enabled = false
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

async fn wait_for_idles(
    seen: &Arc<Mutex<Vec<Option<v2::StopReason>>>>,
    count: usize,
) -> Vec<Option<v2::StopReason>> {
    for _ in 0..400 {
        {
            let seen = seen.lock().unwrap();
            if seen.len() >= count {
                return seen.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    seen.lock().unwrap().clone()
}

fn text(value: &str) -> v2::ContentBlock {
    v2::ContentBlock::Text(v2::TextContent::new(value))
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
    let seen: Arc<Mutex<Vec<Option<v2::StopReason>>>> = Arc::default();
    let handler = Arc::clone(&seen);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                if let v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(idle)) =
                    &notification.update
                {
                    handler.lock().unwrap().push(idle.stop_reason.clone());
                }
                Ok(())
            },
            on_receive_notification!(),
        )
        .connect_with(agent, |connection: V2ConnectionTo<Agent>| async move {
            connection
                .send_request(v2::InitializeRequest::new(
                    ProtocolVersion::V2,
                    v2::Implementation::new("test-client", "1.0.0"),
                ))
                .block_task()
                .await?;
            let session = connection
                .send_request(v2::NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;
            let prompt = connection.send_request(v2::PromptRequest::new(
                session.session_id.clone(),
                vec![text("hello")],
            ));
            // A notification, so it reaches the dispatch loop after the
            // prompt handler that registered the turn's token.
            connection.send_notification(v2::CancelSessionNotification::new(
                session.session_id.clone(),
            ))?;
            let accepted = prompt.block_task().await?;
            assert!(!accepted.message_id.0.is_empty());

            let idles = wait_for_idles(&seen, 1).await;
            assert_eq!(idles.len(), 1, "one terminal idle: {idles:?}");
            assert!(
                matches!(idles[0], Some(v2::StopReason::Cancelled)),
                "cancel yields idle(cancelled): {idles:?}"
            );
            Ok(())
        })
        .await
        .expect("v2 cancelled turn");
}

#[tokio::test(flavor = "multi_thread")]
async fn session_cancel_for_an_idle_session_is_a_noop() {
    let dir = tempfile::tempdir().unwrap();
    // No model request is made, so the endpoint is never contacted.
    write_config(dir.path(), "http://127.0.0.1:1/v1");
    let agent = spawn_agent(dir.path());

    Client
        .v2()
        .connect_with(agent, |connection: V2ConnectionTo<Agent>| async move {
            connection
                .send_request(v2::InitializeRequest::new(
                    ProtocolVersion::V2,
                    v2::Implementation::new("test-client", "1.0.0"),
                ))
                .block_task()
                .await?;
            let session = connection
                .send_request(v2::NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;
            connection.send_notification(v2::CancelSessionNotification::new(
                session.session_id.clone(),
            ))?;
            // The connection is unaffected: the next request round-trips.
            connection
                .send_request(v2::ListSessionsRequest::new())
                .block_task()
                .await?;
            Ok(())
        })
        .await
        .expect("v2 idle cancel is a no-op");
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
    let seen: Arc<Mutex<Vec<Option<v2::StopReason>>>> = Arc::default();
    let handler = Arc::clone(&seen);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                if let v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(idle)) =
                    &notification.update
                {
                    handler.lock().unwrap().push(idle.stop_reason.clone());
                }
                Ok(())
            },
            on_receive_notification!(),
        )
        .connect_with(agent, |connection: V2ConnectionTo<Agent>| async move {
            connection
                .send_request(v2::InitializeRequest::new(
                    ProtocolVersion::V2,
                    v2::Implementation::new("test-client", "1.0.0"),
                ))
                .block_task()
                .await?;
            let session = connection
                .send_request(v2::NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;
            let prompt = connection.send_request(v2::PromptRequest::new(
                session.session_id.clone(),
                vec![text("hello")],
            ));
            let close =
                connection.send_request(v2::CloseSessionRequest::new(session.session_id.clone()));

            let idles = wait_for_idles(&seen, 1).await;
            assert!(
                matches!(idles.first(), Some(Some(v2::StopReason::Cancelled))),
                "close cancels the turn: {idles:?}"
            );
            prompt.block_task().await?;
            close.block_task().await?;

            let listed = connection
                .send_request(v2::ListSessionsRequest::new())
                .block_task()
                .await?;
            assert!(
                listed
                    .sessions
                    .iter()
                    .any(|listed| listed.session_id == session.session_id),
                "close keeps the session listable"
            );
            Ok(())
        })
        .await
        .expect("v2 close during a turn");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_turn_releases_the_lease_for_the_next_prompt() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(SentinelledModel)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), &server.uri());
    let agent = spawn_agent(dir.path());
    let seen: Arc<Mutex<Vec<Option<v2::StopReason>>>> = Arc::default();
    let handler = Arc::clone(&seen);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                if let v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(idle)) =
                    &notification.update
                {
                    handler.lock().unwrap().push(idle.stop_reason.clone());
                }
                Ok(())
            },
            on_receive_notification!(),
        )
        .connect_with(agent, |connection: V2ConnectionTo<Agent>| async move {
            connection
                .send_request(v2::InitializeRequest::new(
                    ProtocolVersion::V2,
                    v2::Implementation::new("test-client", "1.0.0"),
                ))
                .block_task()
                .await?;
            let session = connection
                .send_request(v2::NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;
            let session_id = session.session_id.clone();

            let prompt =
                connection.send_request(v2::PromptRequest::new(session_id.clone(), vec![text("HANG")]));
            connection.send_notification(v2::CancelSessionNotification::new(session_id.clone()))?;
            prompt.block_task().await?;
            let idles = wait_for_idles(&seen, 1).await;
            assert!(
                matches!(idles.first(), Some(Some(v2::StopReason::Cancelled))),
                "the hanging turn is cancelled: {idles:?}"
            );

            // The lease is released, so the next prompt is not refused as
            // session-actively-owned.
            connection
                .send_request(v2::PromptRequest::new(session_id, vec![text("hello")]))
                .block_task()
                .await?;
            let idles = wait_for_idles(&seen, 2).await;
            assert!(
                matches!(idles.get(1), Some(Some(v2::StopReason::EndTurn))),
                "the next turn completes: {idles:?}"
            );
            Ok(())
        })
        .await
        .expect("v2 cancelled turn releases the lease");
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_after_completion_is_a_noop() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(SentinelledModel)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), &server.uri());
    let agent = spawn_agent(dir.path());
    let seen: Arc<Mutex<Vec<Option<v2::StopReason>>>> = Arc::default();
    let handler = Arc::clone(&seen);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                if let v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(idle)) =
                    &notification.update
                {
                    handler.lock().unwrap().push(idle.stop_reason.clone());
                }
                Ok(())
            },
            on_receive_notification!(),
        )
        .connect_with(agent, |connection: V2ConnectionTo<Agent>| async move {
            connection
                .send_request(v2::InitializeRequest::new(
                    ProtocolVersion::V2,
                    v2::Implementation::new("test-client", "1.0.0"),
                ))
                .block_task()
                .await?;
            let session = connection
                .send_request(v2::NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;
            let session_id = session.session_id.clone();

            connection
                .send_request(v2::PromptRequest::new(session_id.clone(), vec![text("hello")]))
                .block_task()
                .await?;
            let idles = wait_for_idles(&seen, 1).await;
            assert!(
                matches!(idles.first(), Some(Some(v2::StopReason::EndTurn))),
                "the turn completes: {idles:?}"
            );

            // A cancel after the turn resolved is a no-op; the connection
            // keeps working.
            connection.send_notification(v2::CancelSessionNotification::new(session_id))?;
            connection
                .send_request(v2::ListSessionsRequest::new())
                .block_task()
                .await?;
            Ok(())
        })
        .await
        .expect("v2 cancel after completion is a no-op");
}
