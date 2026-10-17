//! Client-capability tools (Bug 2): an advertised `fs.read_text_file`
//! surfaces to the model as `client.fs.read_text_file`, the call bridges
//! back over ACP to the client, and the client's answer reaches the next
//! model request as the tool result.

use agentkit_tackle::agent_client_protocol::schema::v1::{
    ClientCapabilities, ContentBlock, FileSystemCapabilities, InitializeRequest, NewSessionRequest,
    PromptRequest, ReadTextFileRequest, ReadTextFileResponse, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome, TextContent,
};
use agentkit_tackle::agent_client_protocol::schema::ProtocolVersion;
use agentkit_tackle::agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo};
use std::path::PathBuf;
use std::sync::{Arc as StdArc, Mutex as StdMutex};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// The tool the advertised capability must surface.
const TOOL: &str = "client.fs.read_text_file";
/// Content the client returns; its presence in a later model request is
/// proof the tool result flowed back.
const MARKER: &str = "EDITOR-CONTENT-MARKER";
/// The path the scripted model asks for.
const PATH: &str = "/work/readme.md";
const PROMPT: &str = "read the file";

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
            serde_json::to_string(&serde_json::json!({ "path": PATH })).expect("arguments");
        if body["stream"].as_bool().unwrap_or(false) {
            let lines = [
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{"role":"assistant"}}]}"#.to_owned(),
                format!(
                    r#"data: {{"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"call_ct","type":"function","function":{{"name":"{TOOL}","arguments":{arguments}}}}}]}}}}]}}"#
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
                            "id": "call_ct",
                            "type": "function",
                            "function": {"name": TOOL, "arguments": arguments},
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
        let raw = body["messages"]
            .as_array()
            .map(|messages| {
                messages
                    .iter()
                    .map(|message| message.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if raw.iter().any(|message| message.contains(MARKER)) {
            Self::answer(&body, "done")
        } else {
            Self::tool_call(&body)
        }
    }
}

fn write_config(dir: &std::path::Path, endpoint_uri: &str) {
    let cfg = dir.join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("config.toml"),
        format!(
            "[endpoints.mock]\nbase_url = \"{endpoint_uri}\"\nwire_format = \"openai-chat-completions\"\nauth = \"none\"\nmodels = [\"MiniMax-M2\"]\n\n[defaults]\nmodel = \"mock/MiniMax-M2\"\n"
        ),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn advertised_fs_read_reaches_the_client_and_returns_to_the_model() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    write_config(dir.path(), &server.uri());

    let agent = AcpAgent::from_args([
        env!("CARGO_BIN_EXE_agentkit-tackle"),
        "stdio",
        "--config-dir",
        dir.path().join("cfg").to_str().unwrap(),
        "--db-path",
        dir.path().join("sessions.db").to_str().unwrap(),
    ])
    .unwrap();

    let read_paths: StdArc<StdMutex<Vec<String>>> = StdArc::default();
    let handler_paths = read_paths.clone();

    Client
        .builder()
        .on_receive_request(
            async move |request: ReadTextFileRequest, responder, _connection| {
                handler_paths
                    .lock()
                    .unwrap()
                    .push(request.path.to_string_lossy().into_owned());
                let _ = responder.respond(ReadTextFileResponse::new(MARKER));
                Ok(())
            },
            agentkit_tackle::agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| {
                match request.options.first() {
                    Some(option) => responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                            option.option_id.clone(),
                        )),
                    )),
                    None => responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Cancelled,
                    )),
                }
            },
            agentkit_tackle::agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(
                    InitializeRequest::new(ProtocolVersion::V1).client_capabilities(
                        ClientCapabilities::new()
                            .fs(FileSystemCapabilities::new().read_text_file(true)),
                    ),
                )
                .block_task()
                .await?;
            let created = connection
                .send_request(NewSessionRequest::new(PathBuf::from("/work")))
                .block_task()
                .await?;
            connection
                .send_request(PromptRequest::new(
                    created.session_id,
                    vec![ContentBlock::Text(TextContent::new(PROMPT))],
                ))
                .block_task()
                .await?;
            Ok(())
        })
        .await
        .expect("client scenario");

    assert_eq!(
        read_paths.lock().unwrap().as_slice(),
        [PATH],
        "the model's call reached the client with the requested path"
    );

    let requests = server
        .received_requests()
        .await
        .expect("mock recorded requests");
    let reached_model = requests.iter().any(|request| {
        String::from_utf8_lossy(&request.body).contains(MARKER)
    });
    assert!(
        reached_model,
        "the file content returned by the client must reach a later model request"
    );
}
