//! v2 forwarded MCP: a server passed on `session/new` joins the pool, is
//! advertised as `!mcp.<server>.*`, and its tool is callable through the
//! permission pipeline.

use agentkit_tackle::agent_client_protocol::schema::v2;
use agentkit_tackle::agent_client_protocol::schema::ProtocolVersion;
use agentkit_tackle::agent_client_protocol::{
    AcpAgent, Agent, Client, V2ConnectionTo, on_receive_notification, on_receive_request,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const SERVER: &str = "fs";
const TOOL: &str = "mcp.fs.read_file";
const MARKER: &str = "FORWARDED-MARKER";

fn usage() -> serde_json::Value {
    serde_json::json!({ "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 })
}

struct ForwardedModel;

impl ForwardedModel {
    fn answer(body: &serde_json::Value, text: &str) -> ResponseTemplate {
        if body["stream"].as_bool().unwrap_or(false) {
            let lines = [
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{"role":"assistant"}}]}"#.to_owned(),
                format!(
                    r#"data: {{"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{{"index":0,"delta":{{"content":{}}}}}]}}"#,
                    serde_json::to_string(text).expect("text serialises")
                ),
                format!(
                    r#"data: {{"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{{"index":0,"delta":{{}},"finish_reason":"stop"}}],"usage":{}}}"#,
                    usage()
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
                    "message": {"role": "assistant", "content": text},
                    "finish_reason": "stop",
                }],
                "usage": usage(),
            }))
        }
    }

    fn tool_call(body: &serde_json::Value) -> ResponseTemplate {
        let arguments = serde_json::to_string(&serde_json::json!({ "path": "note.txt" }))
            .expect("arguments serialise");
        if body["stream"].as_bool().unwrap_or(false) {
            let lines = [
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{"role":"assistant"}}]}"#.to_owned(),
                format!(
                    r#"data: {{"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"call_forwarded","type":"function","function":{{"name":"{TOOL}","arguments":{arguments}}}}}]}}}}]}}"#
                ),
                format!(
                    r#"data: {{"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{{"index":0,"delta":{{}},"finish_reason":"tool_calls"}}],"usage":{}}}"#,
                    usage()
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
                    "message": {
                        "role": "assistant",
                        "content": serde_json::Value::Null,
                        "tool_calls": [{
                            "id": "call_forwarded",
                            "type": "function",
                            "function": {"name": TOOL, "arguments": arguments},
                        }],
                    },
                    "finish_reason": "tool_calls",
                }],
                "usage": usage(),
            }))
        }
    }
}

impl Respond for ForwardedModel {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let messages = body
            .get("messages")
            .and_then(|messages| messages.as_array())
            .cloned()
            .unwrap_or_default();
        let tool_text = messages
            .iter()
            .filter(|message| message.get("role").and_then(|role| role.as_str()) == Some("tool"))
            .map(|message| message.to_string())
            .collect::<String>();
        if tool_text.contains(MARKER) {
            Self::answer(&body, &format!("done: {MARKER}"))
        } else if tool_text.is_empty() {
            Self::tool_call(&body)
        } else {
            Self::answer(&body, "done: missing")
        }
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

#[derive(Debug, Default)]
struct Seen {
    commands: Vec<String>,
    permissions: Vec<String>,
    text: String,
    idles: Vec<Option<v2::StopReason>>,
}

async fn wait_for(seen: &Arc<Mutex<Seen>>, ready: impl Fn(&Seen) -> bool) -> bool {
    for _ in 0..400 {
        if ready(&seen.lock().unwrap()) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forwarded_mcp_server_is_advertised_and_callable() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("note.txt"), format!("{MARKER}\n")).unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ForwardedModel)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), &server.uri());
    let agent = spawn_agent(dir.path());
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let handler = Arc::clone(&seen);
    let asks = Arc::clone(&seen);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                let mut seen = handler.lock().unwrap();
                match &notification.update {
                    v2::SessionUpdate::AvailableCommandsUpdate(update) => {
                        seen.commands = update
                            .available_commands
                            .iter()
                            .map(|command| command.name.clone())
                            .collect();
                    }
                    v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(idle)) => {
                        seen.idles.push(idle.stop_reason.clone());
                    }
                    v2::SessionUpdate::AgentMessageChunk(chunk) => {
                        if let v2::ContentBlock::Text(text) = &chunk.content {
                            seen.text.push_str(&text.text);
                        }
                    }
                    _ => {}
                }
                Ok(())
            },
            on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: v2::RequestPermissionRequest, responder, _cx| {
                asks.lock().unwrap().permissions.push(request.title.clone());
                let chosen = request
                    .options
                    .iter()
                    .find(|option| option.name == "Allow once")
                    .or_else(|| request.options.first());
                match chosen {
                    Some(option) => responder.respond(v2::RequestPermissionResponse::new(
                        v2::RequestPermissionOutcome::Selected(
                            v2::SelectedPermissionOutcome::new(option.option_id.clone()),
                        ),
                    )),
                    None => responder.respond(v2::RequestPermissionResponse::new(
                        v2::RequestPermissionOutcome::Cancelled,
                    )),
                }
            },
            on_receive_request!(),
        )
        .connect_with(agent, |connection: V2ConnectionTo<Agent>| async move {
            connection
                .send_request(v2::InitializeRequest::new(
                    ProtocolVersion::V2,
                    v2::Implementation::new("test-client", "1.0.0"),
                ))
                .block_task()
                .await?;
            let forwarded = v2::McpServer::Stdio(
                v2::McpServerStdio::new(
                    SERVER,
                    std::path::PathBuf::from(env!("CARGO_BIN_EXE_tackle-mcp-fs")),
                )
                .args(vec![
                    "--root".to_owned(),
                    workspace.path().to_string_lossy().into_owned(),
                ]),
            );
            let session = connection
                .send_request(
                    v2::NewSessionRequest::new(std::env::current_dir().unwrap())
                        .mcp_servers(vec![forwarded]),
                )
                .block_task()
                .await?;

            assert!(
                wait_for(&seen, |seen| {
                    seen.commands.iter().any(|name| name == &format!("!{TOOL}"))
                })
                .await,
                "the forwarded tool is advertised: {:?}",
                seen.lock().unwrap().commands
            );

            connection
                .send_request(v2::PromptRequest::new(
                    session.session_id,
                    vec![v2::ContentBlock::Text(v2::TextContent::new(
                        "read note.txt",
                    ))],
                ))
                .block_task()
                .await?;
            assert!(
                wait_for(&seen, |seen| !seen.idles.is_empty()).await,
                "the turn reaches idle: {:?}",
                seen.lock().unwrap()
            );
            let seen = seen.lock().unwrap();
            assert_eq!(
                seen.idles,
                vec![Some(v2::StopReason::EndTurn)],
                "the forwarded tool turn completes: {seen:?}"
            );
            assert_eq!(seen.permissions.len(), 1, "the tool asks: {seen:?}");
            assert!(
                seen.text.contains(MARKER),
                "the forwarded fs tool returned the file: {seen:?}"
            );
            Ok(())
        })
        .await
        .expect("v2 forwarded MCP");
}
