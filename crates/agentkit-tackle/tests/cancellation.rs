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

// A model that hangs on demand: requests whose body contains the HANG
// sentinel never answer, so the turn stays in flight until cancelled.
struct SentinelledModel;

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

#[tokio::test(flavor = "multi_thread")]
async fn cancel_request_resolves_the_prompt_as_cancelled() {
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

            let prompt = connection.send_request(PromptRequest::new(
                session.session_id.clone(),
                vec![ContentBlock::Text(TextContent::new("hello".to_owned()))],
            ));
            // Sends `$/cancel_request` for the in-flight prompt without
            // discarding its response.
            prompt.cancel()?;
            let response = prompt.block_task().await?;
            assert_eq!(response.stop_reason, StopReason::Cancelled);
            Ok(())
        })
        .await
        .expect("cancel request resolves the prompt");
}

impl Respond for SentinelledModel {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap_or_default();
        // The final content part carried by the request: the current user
        // prompt. Prior turns ride earlier parts, so only an exact match
        // hangs — a later prompt in the same session is answered.
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
            return ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(120));
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
            format!("data: {}", chunk(serde_json::json!({ "role": "assistant" }), None)),
            format!("data: {}", chunk(serde_json::json!({ "content": "ok" }), None)),
            format!("data: {}", chunk(serde_json::json!({}), Some("stop"))),
            String::new(),
        ];
        ResponseTemplate::new(200).set_body_raw(lines.join("\n\n") + "\n", "text/event-stream")
    }
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
                vec![ContentBlock::Text(TextContent::new("HANG".to_owned()))],
            ));
            connection.send_notification(CancelNotification::new(session_id.clone()))?;
            let cancelled = prompt.block_task().await?;
            assert_eq!(cancelled.stop_reason, StopReason::Cancelled);

            // The lease is released, so the next prompt is not refused as
            // session-actively-owned.
            let next = connection
                .send_request(PromptRequest::new(
                    session_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("hello".to_owned()))],
                ))
                .block_task()
                .await?;
            assert_eq!(next.stop_reason, StopReason::EndTurn);
            Ok(())
        })
        .await
        .expect("cancelled turn releases the lease");
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

            let response = connection
                .send_request(PromptRequest::new(
                    session_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("hello".to_owned()))],
                ))
                .block_task()
                .await?;
            assert_eq!(response.stop_reason, StopReason::EndTurn);

            // A cancel after the turn resolved is a no-op; the connection
            // keeps working.
            connection.send_notification(CancelNotification::new(session_id.clone()))?;
            connection
                .send_request(ListSessionsRequest::new())
                .block_task()
                .await?;
            Ok(())
        })
        .await
        .expect("cancel after completion is a no-op");
}

/// A model endpoint that answers `GET /models` (so session setup does not
/// stall) and, for a completion, streams one chunk then holds the
/// connection until the test releases a second chunk. That makes the
/// no-update-after-cancel assertion real: the stream is provably active
/// before the cancel, and the fixture provably tries to send more after it.
fn streaming_fixture() -> (String, std::sync::mpsc::Sender<()>) {
    use std::io::Write as _;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (release, released) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut head = [0u8; 1024];
            let peeked = stream.peek(&mut head).unwrap_or(0);
            if peeked == 0 {
                continue;
            }
            let request_line = String::from_utf8_lossy(&head[..peeked]);
            if request_line.starts_with("GET ") {
                let body = r#"{"data":[{"id":"gpt-4o-mini"}]}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                continue;
            }
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\n\r\n",
            );
            let _ = stream.write_all(sse_chunk("one").as_bytes());
            let _ = stream.flush();
            if released.recv().is_ok() {
                let _ = stream.write_all(sse_chunk("two").as_bytes());
                let _ = stream.flush();
            }
            held.push(stream);
        }
    });
    (format!("http://{addr}/v1"), release)
}

/// One OpenAI-compatible SSE content chunk.
fn sse_chunk(text: &str) -> String {
    format!(
        "data: {}\n\n",
        serde_json::json!({
            "id": "c1",
            "object": "chat.completion.chunk",
            "created": 0,
            "model": "m",
            "choices": [{ "index": 0, "delta": { "content": text } }],
        })
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn no_content_update_follows_the_cancelled_response() {
    use agentkit_tackle::agent_client_protocol::schema::v1::{SessionNotification, SessionUpdate};
    use std::sync::{Arc as StdArc, Mutex as StdMutex};
    use std::time::{Duration, Instant};

    let (base_url, release) = streaming_fixture();
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), &base_url);
    let agent = spawn_agent(dir.path());

    let seen: StdArc<StdMutex<Vec<String>>> = StdArc::new(StdMutex::new(Vec::new()));
    let handler = seen.clone();

    Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                if let SessionUpdate::AgentMessageChunk(chunk) = &notification.update {
                    if let ContentBlock::Text(text) = &chunk.content {
                        handler.lock().unwrap().push(text.text.clone());
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
            let session = connection
                .send_request(NewSessionRequest::new(std::path::PathBuf::from("/work")))
                .block_task()
                .await?;
            let session_id = session.session_id.clone();

            let prompt = connection.send_request(PromptRequest::new(
                session_id.clone(),
                vec![ContentBlock::Text(TextContent::new("hello".to_owned()))],
            ));

            // Wait until the stream is demonstrably active.
            let deadline = Instant::now() + Duration::from_secs(5);
            while seen.lock().unwrap().is_empty() && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(
                seen.lock().unwrap().clone(),
                vec!["one".to_owned()],
                "the stream produced a chunk before cancellation"
            );

            connection.send_notification(CancelNotification::new(session_id.clone()))?;
            let response = prompt.block_task().await?;
            assert_eq!(response.stop_reason, StopReason::Cancelled);

            // Release the fixture's second chunk; cancellation must have
            // closed the stream, so it never reaches the client.
            let _ = release.send(());
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert_eq!(
                seen.lock().unwrap().clone(),
                vec!["one".to_owned()],
                "no content update follows the cancelled response"
            );
            Ok(())
        })
        .await
        .expect("no late updates");
}
