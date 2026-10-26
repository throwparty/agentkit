//! v2 permission coverage: a scripted tool call blocks on
//! `session/request_permission`, the client allows it, and the foreground
//! state moves running -> requires_action -> running before the tool runs.

use agentkit_tackle::agent_client_protocol::schema::v2;
use agentkit_tackle::agent_client_protocol::schema::ProtocolVersion;
use agentkit_tackle::agent_client_protocol::{
    AcpAgent, Agent, Client, V2ConnectionTo, on_receive_notification, on_receive_request,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const TOOL: &str = "mcp.echo.echo";

fn usage() -> serde_json::Value {
    serde_json::json!({ "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 })
}

struct ToolModel;

impl ToolModel {
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

    fn tool_call(body: &serde_json::Value, arg: &str) -> ResponseTemplate {
        let arguments = serde_json::to_string(&serde_json::json!({ "text": arg }))
            .expect("arguments serialise");
        if body["stream"].as_bool().unwrap_or(false) {
            let lines = [
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{"role":"assistant"}}]}"#.to_owned(),
                format!(
                    r#"data: {{"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"call_e2e","type":"function","function":{{"name":"{TOOL}","arguments":{arguments}}}}}]}}}}]}}"#
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
                            "id": "call_e2e",
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

impl Respond for ToolModel {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let messages = body
            .get("messages")
            .and_then(|messages| messages.as_array())
            .cloned()
            .unwrap_or_default();
        let has_tool_result = messages
            .iter()
            .any(|message| message.get("role").and_then(|role| role.as_str()) == Some("tool"));
        if has_tool_result {
            Self::answer(&body, "done")
        } else {
            Self::tool_call(&body, "ping")
        }
    }
}

fn write_config(dir: &std::path::Path, endpoint_uri: &str) {
    let cfg = dir.join("cfg");
    std::fs::create_dir_all(cfg.join("scripts")).unwrap();
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

[mcp_servers.echo]
transport = "stdio"
command = {echo}

[scripts.veto]
events = ["pre_tool_use"]
file = "veto.rhai"

[scripts.titling]
events = ["title_trigger"]
file = "builtin:titling.rhai"
enabled = false
"#,
            echo = serde_json::to_string(env!("CARGO_BIN_EXE_tackle-mcp-echo"))
                .expect("path serialises"),
        ),
    )
    .unwrap();
    std::fs::write(
        cfg.join("scripts").join("veto.rhai"),
        "fn pre_tool_use(request) { #{ decision: \"ask\" } }\n",
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
    transitions: Vec<&'static str>,
    permissions: Vec<PermissionSeen>,
    text: String,
    idles: Vec<Option<v2::StopReason>>,
}

#[derive(Debug)]
struct PermissionSeen {
    title: String,
    tool_call_id: String,
    options: Vec<String>,
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
async fn v2_permission_request_shapes_the_ask_and_foreground_states() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ToolModel)
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
                    v2::SessionUpdate::StateUpdate(v2::StateUpdate::Running(_)) => {
                        seen.transitions.push("running");
                    }
                    v2::SessionUpdate::StateUpdate(v2::StateUpdate::RequiresAction(_)) => {
                        seen.transitions.push("requires_action");
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
                let tool_call_id = match &request.subject {
                    Some(v2::RequestPermissionSubject::ToolCall(subject)) => {
                        subject.tool_call.tool_call_id.0.to_string()
                    }
                    _ => String::new(),
                };
                let options = request
                    .options
                    .iter()
                    .map(|option| option.name.clone())
                    .collect::<Vec<_>>();
                asks.lock().unwrap().permissions.push(PermissionSeen {
                    title: request.title.clone(),
                    tool_call_id,
                    options,
                });
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
            let session = connection
                .send_request(v2::NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;
            connection
                .send_request(v2::PromptRequest::new(
                    session.session_id,
                    vec![v2::ContentBlock::Text(v2::TextContent::new(
                        "use the echo tool to echo ping",
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
                "the allowed tool turn completes: {seen:?}"
            );
            assert_eq!(
                seen.permissions.len(),
                1,
                "exactly one permission ask: {seen:?}"
            );
            let ask = &seen.permissions[0];
            assert!(
                !ask.title.is_empty(),
                "the permission ask carries a title: {ask:?}"
            );
            assert!(
                !ask.tool_call_id.is_empty(),
                "the ask names the in-flight tool call: {ask:?}"
            );
            assert!(
                ask.options.iter().any(|name| name == "Allow once"),
                "the ask carries the standard options: {ask:?}"
            );
            assert_eq!(
                seen.transitions,
                vec!["running", "requires_action", "running"],
                "the foreground moves running -> requires_action -> running"
            );
            assert_eq!(seen.text, "done", "the tool result feeds the next turn");
            Ok(())
        })
        .await
        .expect("v2 permission ask");
}
