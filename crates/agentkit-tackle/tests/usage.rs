//! End-to-end usage and `/context` coverage over ACP: cost-bearing and
//! unpriced endpoints, one snapshot per lifecycle path, unknown-window
//! silence, and the ephemeral report.

use agentkit_tackle::agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, ListSessionsRequest, LoadSessionRequest, NewSessionRequest,
    PromptRequest, ResumeSessionRequest, SessionNotification, SessionUpdate, StopReason,
    TextContent,
};
use agentkit_tackle::agent_client_protocol::schema::ProtocolVersion;
use agentkit_tackle::agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo};
use std::sync::{Arc as StdArc, Mutex as StdMutex};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const PROMPT_TOKENS: u64 = 1_000;
const COMPLETION_TOKENS: u64 = 500;

#[derive(Clone, Debug, Default)]
struct UsageRecord {
    session: String,
    used: u64,
    size: u64,
    cost: Option<f64>,
}

struct ScriptedModel;

impl Respond for ScriptedModel {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let usage = serde_json::json!({
            "prompt_tokens": PROMPT_TOKENS,
            "completion_tokens": COMPLETION_TOKENS,
            "total_tokens": PROMPT_TOKENS + COMPLETION_TOKENS,
        });
        if body["stream"].as_bool().unwrap_or(false) {
            let lines = [
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{"role":"assistant"}}]}"#.to_owned(),
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{"content":"ok"}}]}"#.to_owned(),
                format!(
                    r#"data: {{"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{{"index":0,"delta":{{}},"finish_reason":"stop"}}],"usage":{usage}}}"#
                ),
                "data: [DONE]".to_owned(),
            ];
            ResponseTemplate::new(200).set_body_raw(lines.join("\n\n") + "\n", "text/event-stream")
        } else {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "c1",
                "object": "chat.completion",
                "created": 0,
                "model": "m",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "ok"},
                    "finish_reason": "stop",
                }],
                "usage": usage,
            }))
        }
    }
}

fn spawn_agent(dir: &std::path::Path, config: &str) -> AcpAgent {
    let cfg = dir.join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("config.toml"), config).unwrap();
    AcpAgent::from_args([
        env!("CARGO_BIN_EXE_agentkit-tackle"),
        "stdio",
        "--config-dir",
        cfg.to_str().unwrap(),
        "--db-path",
        dir.join("sessions.db").to_str().unwrap(),
    ])
    .unwrap()
}

fn titling_disabled() -> &'static str {
    "[scripts.titling]\n\
     events = [\"title_trigger\"]\n\
     file = \"builtin:titling.rhai\"\n\
     enabled = false\n\n"
}

#[tokio::test(flavor = "multi_thread")]
async fn priced_usage_carries_cost_and_each_lifecycle_path_sends_one_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    let agent = spawn_agent(
        dir.path(),
        &format!(
            "[endpoints.openai]\nbase_url = \"{}\"\nwire_format = \"openai-chat-completions\"\nauth = \"none\"\nmodels = [\"gpt-4o\"]\n\n\
             [defaults]\nmodel = \"openai/gpt-4o\"\n\n{}",
            server.uri(),
            titling_disabled()
        ),
    );

    let usage: StdArc<StdMutex<Vec<UsageRecord>>> = StdArc::default();
    let handler = usage.clone();
    Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                if let SessionUpdate::UsageUpdate(update) = &notification.update {
                    handler.lock().unwrap().push(UsageRecord {
                        session: notification.session_id.0.to_string(),
                        used: update.used,
                        size: update.size,
                        cost: update.cost.as_ref().map(|cost| cost.amount),
                    });
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
            let session = created.session_id.0.to_string();
            connection
                .send_request(ListSessionsRequest::new())
                .block_task()
                .await?;
            {
                let seen = usage.lock().unwrap();
                assert_eq!(seen.len(), 1, "one snapshot after session/new: {seen:?}");
                assert_eq!(seen[0].session, session);
                assert!(seen[0].size > 0, "known window: {seen:?}");
                assert!(seen[0].cost.is_none(), "no model call yet: {seen:?}");
            }

            usage.lock().unwrap().clear();
            let response = connection
                .send_request(PromptRequest::new(
                    created.session_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("hello"))],
                ))
                .block_task()
                .await?;
            assert_eq!(response.stop_reason, StopReason::EndTurn);
            connection
                .send_request(ListSessionsRequest::new())
                .block_task()
                .await?;
            {
                let seen = usage.lock().unwrap();
                assert_eq!(seen.len(), 1, "one update for one model request: {seen:?}");
                assert_eq!(seen[0].used, PROMPT_TOKENS);
                assert!(seen[0].size >= PROMPT_TOKENS);
                let cost = seen[0].cost.expect("priced endpoint reports cost");
                assert!(
                    (cost - 0.0075).abs() < 1e-9,
                    "1000 input + 500 output at gpt-4o rates: {cost}"
                );
            }

            usage.lock().unwrap().clear();
            connection
                .send_request(ResumeSessionRequest::new(
                    created.session_id.clone(),
                    std::path::PathBuf::from("/work"),
                ))
                .block_task()
                .await?;
            connection
                .send_request(ListSessionsRequest::new())
                .block_task()
                .await?;
            {
                let seen = usage.lock().unwrap();
                assert_eq!(seen.len(), 1, "one snapshot after session/resume: {seen:?}");
                assert_eq!(seen[0].session, session);
                assert!(
                    seen[0].cost.is_some_and(|cost| cost > 0.0),
                    "resume forwards the cumulative cost: {seen:?}"
                );
            }

            usage.lock().unwrap().clear();
            connection
                .send_request(LoadSessionRequest::new(
                    created.session_id.clone(),
                    std::path::PathBuf::from("/work"),
                ))
                .block_task()
                .await?;
            connection
                .send_request(ListSessionsRequest::new())
                .block_task()
                .await?;
            {
                let seen = usage.lock().unwrap();
                assert_eq!(seen.len(), 1, "one snapshot after session/load: {seen:?}");
                assert_eq!(seen[0].session, session);
                assert!(
                    seen[0].cost.is_some_and(|cost| cost > 0.0),
                    "load forwards the cumulative cost: {seen:?}"
                );
            }
            Ok(())
        })
        .await
        .expect("priced lifecycle");
}

#[tokio::test(flavor = "multi_thread")]
async fn unpriced_endpoint_omits_cost() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    let agent = spawn_agent(
        dir.path(),
        &format!(
            "[endpoints.switchboard]\nbase_url = \"{}\"\nwire_format = \"openai-chat-completions\"\nauth = \"none\"\nmodels = [\"gpt-4o\"]\n\n\
             [defaults]\nmodel = \"switchboard/gpt-4o\"\n\n{}",
            server.uri(),
            titling_disabled()
        ),
    );

    let usage: StdArc<StdMutex<Vec<UsageRecord>>> = StdArc::default();
    let handler = usage.clone();
    Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                if let SessionUpdate::UsageUpdate(update) = &notification.update {
                    handler.lock().unwrap().push(UsageRecord {
                        session: notification.session_id.0.to_string(),
                        used: update.used,
                        size: update.size,
                        cost: update.cost.as_ref().map(|cost| cost.amount),
                    });
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
            // Order the session/new snapshot before clearing the recorder.
            connection
                .send_request(ListSessionsRequest::new())
                .block_task()
                .await?;
            usage.lock().unwrap().clear();
            connection
                .send_request(PromptRequest::new(
                    created.session_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("hello"))],
                ))
                .block_task()
                .await?;
            connection
                .send_request(ListSessionsRequest::new())
                .block_task()
                .await?;
            let seen = usage.lock().unwrap();
            assert_eq!(seen.len(), 1, "one update: {seen:?}");
            assert_eq!(seen[0].used, PROMPT_TOKENS);
            assert!(seen[0].size > 0, "the bare model name has a window");
            assert!(seen[0].cost.is_none(), "provider key is unpriced: {seen:?}");
            Ok(())
        })
        .await
        .expect("unpriced prompt");
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_window_is_silent_and_the_report_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let agent = spawn_agent(
        dir.path(),
        &format!(
            "[endpoints.dead]\nbase_url = \"http://127.0.0.1:1\"\nwire_format = \"openai-chat-completions\"\nauth = \"none\"\nmodels = [\"mystery-model\"]\n\n\
             [defaults]\nmodel = \"dead/mystery-model\"\n\n{}",
            titling_disabled()
        ),
    );

    let usage: StdArc<StdMutex<usize>> = StdArc::default();
    let chunks: StdArc<StdMutex<Vec<String>>> = StdArc::default();
    let usage_handler = usage.clone();
    let chunks_handler = chunks.clone();
    Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                match &notification.update {
                    SessionUpdate::UsageUpdate(_) => *usage_handler.lock().unwrap() += 1,
                    SessionUpdate::AgentMessageChunk(chunk) => {
                        if let ContentBlock::Text(text) = &chunk.content {
                            chunks_handler.lock().unwrap().push(text.text.clone());
                        }
                    }
                    _ => {}
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
            connection
                .send_request(ListSessionsRequest::new())
                .block_task()
                .await?;
            assert_eq!(*usage.lock().unwrap(), 0, "unknown window stays silent");

            let response = connection
                .send_request(PromptRequest::new(
                    created.session_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("/context"))],
                ))
                .block_task()
                .await?;
            assert_eq!(response.stop_reason, StopReason::EndTurn);
            connection
                .send_request(ListSessionsRequest::new())
                .block_task()
                .await?;
            let report = chunks.lock().unwrap().join("");
            assert!(
                report.contains("Context window: ~") && report.contains("(window unknown)"),
                "{report}"
            );
            assert!(!report.contains("\nFree"), "{report}");
            assert_eq!(*usage.lock().unwrap(), 0, "/context sends no usage update");
            Ok(())
        })
        .await
        .expect("unknown-window report");
}
