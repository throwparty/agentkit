//! Agent invocation integration test: a model tool call to
//! `agent.<actor>` spawns a nested agent instance of that actor and the
//! nested final message returns as the tool result. Before the fix the
//! invokable was advertised to the model but every call errored.

use agentkit_tackle::agent_client_protocol::schema::ProtocolVersion;
use agentkit_tackle::agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionNotification, SessionUpdate, TextContent,
};
use agentkit_tackle::agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo};
use agentkit_tackle::store::{SessionKind, SessionStore};
use std::sync::{Arc as StdArc, Mutex as StdMutex};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const WORKER_MARKER: &str = "WORKER-PERSONA-MARKER";

struct ScriptedModel;

impl ScriptedModel {
    fn answer(body: &serde_json::Value, text: &str) -> ResponseTemplate {
        if body["stream"].as_bool().unwrap_or(false) {
            let lines = [
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{"role":"assistant"}}]}"#.to_owned(),
                format!(
                    r#"data: {{"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{{"index":0,"delta":{{"content":{}}}}}]}}"#,
                    serde_json::to_string(text).expect("text serialises")
                ),
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#.to_owned(),
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
                    "message": {"role": "assistant", "content": text},
                    "finish_reason": "stop",
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
            }))
        }
    }

    fn tool_call(body: &serde_json::Value) -> ResponseTemplate {
        let arguments =
            serde_json::to_string(&serde_json::json!({ "prompt": "do the nested thing" }))
                .expect("arguments serialise");
        if body["stream"].as_bool().unwrap_or(false) {
            let lines = [
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{"role":"assistant"}}]}"#.to_owned(),
                format!(
                    r#"data: {{"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"call_agent","type":"function","function":{{"name":"agent.worker","arguments":{arguments}}}}}]}}}}]}}"#
                ),
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#.to_owned(),
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
                    "message": {
                        "role": "assistant",
                        "content": serde_json::Value::Null,
                        "tool_calls": [{
                            "id": "call_agent",
                            "type": "function",
                            "function": {"name": "agent.worker", "arguments": arguments},
                        }],
                    },
                    "finish_reason": "tool_calls",
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
            }))
        }
    }
}

impl Respond for ScriptedModel {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let messages = body
            .get("messages")
            .and_then(|messages| messages.as_array())
            .cloned()
            .unwrap_or_default();
        let system = messages
            .iter()
            .filter(|message| message.get("role").and_then(|role| role.as_str()) == Some("system"))
            .map(|message| message.to_string())
            .collect::<Vec<_>>()
            .join("");
        // The nested instance's request carries the worker persona: answer
        // it, proving the nested loop ran.
        if system.contains(WORKER_MARKER) {
            return Self::answer(&body, "nested: done");
        }
        // The parent turn: once its tool result is in the history, answer;
        // otherwise issue the agent tool call.
        if messages
            .iter()
            .any(|message| message.get("role").and_then(|role| role.as_str()) == Some("tool"))
        {
            return Self::answer(&body, "parent: done");
        }
        Self::tool_call(&body)
    }
}

fn write_config(dir: &std::path::Path, endpoint_uri: &str) {
    let cfg = dir.join("cfg");
    std::fs::create_dir_all(cfg.join("personas")).unwrap();
    std::fs::create_dir_all(cfg.join("actors")).unwrap();
    std::fs::write(
        cfg.join("config.toml"),
        format!(
            "[endpoints.mock]\nbase_url = \"{endpoint_uri}\"\nwire_format = \"openai-chat-completions\"\nauth = \"none\"\nmodels = [\"MiniMax-M2\"]\n\n[defaults]\nmodel = \"mock/MiniMax-M2\"\n"
        ),
    )
    .unwrap();
    std::fs::write(
        cfg.join("personas").join("worker.md"),
        format!("+++\ndescription = \"Test worker persona\"\n+++\n{WORKER_MARKER}\n"),
    )
    .unwrap();
    std::fs::write(
        cfg.join("actors").join("worker.md"),
        "+++\npersona = \"worker\"\n+++\n",
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_invocation_returns_the_nested_final_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), &server.uri());
    let db_path = dir.path().join("sessions.db");

    let agent = AcpAgent::from_args([
        env!("CARGO_BIN_EXE_agentkit-tackle"),
        "stdio",
        "--config-dir",
        dir.path().join("cfg").to_str().unwrap(),
        "--db-path",
        db_path.to_str().unwrap(),
    ])
    .expect("agent arguments");

    let results: StdArc<StdMutex<Vec<String>>> = StdArc::default();
    let results_handler = results.clone();

    Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                if let SessionUpdate::ToolCallUpdate(update) = &notification.update {
                    if let Some(content) = &update.fields.content {
                        results_handler
                            .lock()
                            .unwrap()
                            .push(serde_json::to_string(content).unwrap_or_default());
                    }
                }
                Ok(())
            },
            agentkit_tackle::agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _cx| match request
                .options
                .first()
            {
                Some(option) => responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                        option.option_id.clone(),
                    )),
                )),
                None => responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Cancelled,
                )),
            },
            agentkit_tackle::agent_client_protocol::on_receive_request!(),
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
                .send_request(PromptRequest::new(
                    created.session_id.clone(),
                    vec![ContentBlock::Text(TextContent::new(
                        "spawn the worker".to_owned(),
                    ))],
                ))
                .block_task()
                .await?;
            Ok(())
        })
        .await
        .expect("agent invocation round-trip");

    let results = results.lock().unwrap().clone();
    assert!(
        results.iter().any(|result| result.contains("nested: done")),
        "the nested final message became the tool result: {results:?}"
    );

    // The nested instance is an ephemeral fork of the parent session.
    let db = SessionStore::connect_sqlite(&db_path).await.unwrap();
    let sessions = db
        .list_sessions(&agentkit_tackle::store::ListFilter {
            include_ephemeral: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        sessions
            .iter()
            .any(|session| session.kind == SessionKind::Ephemeral),
        "an ephemeral nested session exists: {sessions:?}"
    );
}
