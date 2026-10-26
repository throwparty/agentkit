//! The v1/v2 boundary over one shared store: a v1 turn's lease blocks a v2
//! prompt on the same session, v1 updates never cross into the v2 wire, and
//! a v2 resume replays the v1-authored history.
//!
//! The two agents are spawned sequentially — v1 first, then v2 once v1 is
//! idle — so their SQLite WAL setup does not race. A v2 task probes the
//! session while the v1 turn is in flight.

use agentkit_tackle::agent_client_protocol::schema::v1;
use agentkit_tackle::agent_client_protocol::schema::v2;
use agentkit_tackle::agent_client_protocol::schema::{MaybeUndefined, ProtocolVersion};
use agentkit_tackle::agent_client_protocol::{
    AcpAgent, Agent, Client, ConnectionTo, V2ConnectionTo,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, oneshot};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Hangs only on the `HANG` prompt; every other prompt answers `ok`. The
/// `started` notification proves the hanging turn reached the model, so its
/// lease is held before the other version probes it.
struct SentinelledModel {
    started: Arc<Notify>,
}

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
            self.started.notify_one();
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

#[derive(Default)]
struct V2Seen {
    user_text: String,
    idles: Vec<Option<v2::StopReason>>,
}

async fn wait_until(ready: impl Fn() -> bool) {
    for _ in 0..400 {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn v1_and_v2_share_the_store_and_leases_without_crossing_wire_variants() {
    let server = MockServer::start().await;
    let model_started = Arc::new(Notify::new());
    Mock::given(method("POST"))
        .respond_with(SentinelledModel {
            started: Arc::clone(&model_started),
        })
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), &server.uri());

    let session_slot: Arc<Mutex<Option<String>>> = Arc::default();
    let (v1_new_tx, v1_new_rx) = oneshot::channel();
    let (go_tx, go_rx) = oneshot::channel();
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let (v1_done_tx, v1_done_rx) = oneshot::channel();
    let (v2_init_tx, v2_init_rx) = oneshot::channel();

    // v1 opens the store and creates the session while idle; v2 is spawned
    // only after that, so the two processes' WAL setup is sequential.
    let v1_agent = spawn_agent(dir.path());
    let v1_task = tokio::spawn({
        let session_slot = Arc::clone(&session_slot);
        async move {
            Client
                .builder()
                .connect_with(v1_agent, |connection: ConnectionTo<Agent>| async move {
                    connection
                        .send_request(v1::InitializeRequest::new(ProtocolVersion::V1))
                        .block_task()
                        .await?;
                    let session = connection
                        .send_request(v1::NewSessionRequest::new(std::path::PathBuf::from(
                            "/work",
                        )))
                        .block_task()
                        .await?;
                    *session_slot.lock().unwrap() = Some(session.session_id.to_string());
                    let _ = v1_new_tx.send(());
                    let _ = go_rx.await;
                    let prompt = connection.send_request(v1::PromptRequest::new(
                        session.session_id.clone(),
                        vec![v1::ContentBlock::Text(v1::TextContent::new("HANG"))],
                    ));
                    let _ = cancel_rx.await;
                    connection.send_notification(v1::CancelNotification::new(
                        session.session_id.clone(),
                    ))?;
                    let response = prompt.block_task().await?;
                    assert_eq!(response.stop_reason, v1::StopReason::Cancelled);
                    let _ = v1_done_tx.send(());
                    Ok(())
                })
                .await
        }
    });

    v1_new_rx.await.expect("v1 session created");
    let v2_agent = spawn_agent(dir.path());
    let v2_seen: Arc<Mutex<V2Seen>> = Arc::default();
    let v2_seen_handler = Arc::clone(&v2_seen);
    let v2_task = tokio::spawn({
        let session_slot = Arc::clone(&session_slot);
        async move {
            Client
                .v2()
                .on_receive_notification(
                    async move |notification: v2::UpdateSessionNotification, _cx| {
                        let mut seen = v2_seen_handler.lock().unwrap();
                        match &notification.update {
                            v2::SessionUpdate::UserMessage(message) => {
                                if let MaybeUndefined::Value(blocks) = &message.content {
                                    for block in blocks {
                                        if let v2::ContentBlock::Text(text) = block {
                                            seen.user_text.push_str(&text.text);
                                            seen.user_text.push('\n');
                                        }
                                    }
                                }
                            }
                            v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(idle)) => {
                                seen.idles.push(idle.stop_reason.clone());
                            }
                            _ => {}
                        }
                        Ok(())
                    },
                    agentkit_tackle::agent_client_protocol::on_receive_notification!(),
                )
                .connect_with(v2_agent, |connection: V2ConnectionTo<Agent>| async move {
                    connection
                        .send_request(v2::InitializeRequest::new(
                            ProtocolVersion::V2,
                            v2::Implementation::new("test-client", "1.0.0"),
                        ))
                        .block_task()
                        .await?;
                    let _ = v2_init_tx.send(());
                    // The v1 turn's model request proves its lease is held.
                    model_started.notified().await;
                    let session_id = v2::SessionId::new(
                        session_slot
                            .lock()
                            .unwrap()
                            .clone()
                            .expect("the v1 session id"),
                    );
                    let listed = connection
                        .send_request(v2::ListSessionsRequest::new())
                        .block_task()
                        .await?;
                    assert!(
                        listed
                            .sessions
                            .iter()
                            .any(|session| session.session_id == session_id),
                        "the v1 session is visible to v2"
                    );

                    // The v1 turn holds the shared lease: v2 must refuse.
                    let error = match connection
                        .send_request(v2::PromptRequest::new(
                            session_id.clone(),
                            vec![v2::ContentBlock::Text(v2::TextContent::new("hello"))],
                        ))
                        .block_task()
                        .await
                    {
                        Ok(_) => {
                            panic!("a lease held by another connection must refuse the prompt")
                        }
                        Err(error) => error,
                    };
                    assert_eq!(
                        error
                            .data
                            .as_ref()
                            .and_then(|data| data.get("error"))
                            .and_then(|value| value.as_str()),
                        Some("session-actively-owned"),
                        "the refusal names the shared lease: {error:?}"
                    );

                    let _ = cancel_tx.send(());
                    let _ = v1_done_rx.await;
                    connection
                        .send_request(v2::PromptRequest::new(
                            session_id.clone(),
                            vec![v2::ContentBlock::Text(v2::TextContent::new("hello"))],
                        ))
                        .block_task()
                        .await?;
                    wait_until(|| !v2_seen.lock().unwrap().idles.is_empty()).await;
                    assert_eq!(
                        v2_seen.lock().unwrap().idles,
                        vec![Some(v2::StopReason::EndTurn)],
                        "the v2 turn completes after the v1 cancel"
                    );

                    connection
                        .send_request(
                            v2::ResumeSessionRequest::new(
                                session_id,
                                std::path::PathBuf::from("/work"),
                            )
                            .replay_from(v2::ReplayFrom::Start(v2::ReplayFromStart::new())),
                        )
                        .block_task()
                        .await?;
                    wait_until(|| v2_seen.lock().unwrap().idles.len() >= 2).await;
                    let text = v2_seen.lock().unwrap().user_text.clone();
                    assert!(
                        text.contains("HANG") && text.contains("hello"),
                        "v2 replays the v1-authored history: {text:?}"
                    );
                    Ok(())
                })
                .await
        }
    });

    v2_init_rx.await.expect("v2 initialized");
    let _ = go_tx.send(());
    v2_task.await.expect("v2 task").expect("v2 boundary side");
    v1_task.await.expect("v1 task").expect("v1 boundary side");
}
