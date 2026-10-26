//! ACP v2 integration coverage: version negotiation, a full session lifecycle,
//! a streamed prompt turn with the foreground state machine, and the
//! v1/v2 boundary.

use agentkit_tackle::agent_client_protocol::schema::v2;
use agentkit_tackle::agent_client_protocol::schema::ProtocolVersion;
use agentkit_tackle::agent_client_protocol::{
    AcpAgent, Agent, Client, ConnectionTo, V2ConnectionTo, on_receive_notification,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

struct ScriptedModel;

/// Answers model discovery with no models, so the first prompt has
/// nothing to resolve and fails after its user message is inserted.
struct EmptyModels;

impl Respond for EmptyModels {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": [] }))
    }
}

/// Holds every model request open, so a turn is in flight when close or
/// delete lands.
struct HangingModel;

impl Respond for HangingModel {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        ResponseTemplate::new(200).set_delay(Duration::from_secs(120))
    }
}

impl Respond for ScriptedModel {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap_or_default();
        if body["stream"].as_bool().unwrap_or(false) {
            let lines = [
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{"role":"assistant"}}]}"#,
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{"content":"ok"}}]}"#,
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
                "data: [DONE]",
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

fn model_config(uri: &str) -> String {
    format!(
        "[endpoints.openai]\nbase_url = \"{uri}\"\nwire_format = \"openai-chat-completions\"\nauth = \"none\"\nmodels = [\"gpt-4o\"]\n\n\
         [defaults]\nmodel = \"openai/gpt-4o\"\n\n\
         [scripts.titling]\nevents = [\"title_trigger\"]\nfile = \"builtin:titling.rhai\"\nenabled = false\n"
    )
}

#[derive(Debug, Clone)]
enum Seen {
    User(String),
    Running,
    Idle(Option<v2::StopReason>),
    AgentText(String),
    Compaction(v2::CompactionStatus),
    ToolText(String),
}

async fn wait_for_user_message(seen: &Arc<Mutex<Vec<Seen>>>) -> Option<String> {
    for _ in 0..200 {
        if let Some(Seen::User(id)) = seen
            .lock()
            .unwrap()
            .iter()
            .find(|entry| matches!(entry, Seen::User(_)))
        {
            return Some(id.clone());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    None
}

async fn wait_for_idle(seen: &Arc<Mutex<Vec<Seen>>>) -> Option<v2::StopReason> {
    for _ in 0..200 {
        if let Some(Seen::Idle(reason)) = seen
            .lock()
            .unwrap()
            .iter()
            .find(|entry| matches!(entry, Seen::Idle(_)))
        {
            return reason.clone();
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    None
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_session_lifecycle_and_prompt_drive_a_turn() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    let agent = spawn_agent(dir.path(), &model_config(&server.uri()));

    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let handler = Arc::clone(&seen);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                let mut seen = handler.lock().unwrap();
                match &notification.update {
                    v2::SessionUpdate::UserMessage(message) => {
                        seen.push(Seen::User(message.message_id.to_string()));
                    }
                    v2::SessionUpdate::AgentMessageChunk(chunk) => {
                        if let v2::ContentBlock::Text(text) = &chunk.content {
                            seen.push(Seen::AgentText(text.text.clone()));
                        }
                    }
                    v2::SessionUpdate::StateUpdate(v2::StateUpdate::Running(_)) => {
                        seen.push(Seen::Running);
                    }
                    v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(idle)) => {
                        seen.push(Seen::Idle(idle.stop_reason.clone()));
                    }
                    v2::SessionUpdate::CompactionUpdate(update) => {
                        seen.push(Seen::Compaction(update.status.clone()));
                    }
                    _ => {}
                }
                Ok(())
            },
            on_receive_notification!(),
        )
        .connect_with(agent, |connection: V2ConnectionTo<Agent>| async move {
            let init = connection
                .send_request(v2::InitializeRequest::new(
                    ProtocolVersion::V2,
                    v2::Implementation::new("test-client", "1.0.0"),
                ))
                .block_task()
                .await?;
            assert_eq!(init.protocol_version, ProtocolVersion::V2);

            let cwd = std::env::current_dir().unwrap();
            let created = connection
                .send_request(v2::NewSessionRequest::new(cwd.clone()))
                .block_task()
                .await?;
            assert!(!created.session_id.0.is_empty());
            assert!(!created.available_commands.is_empty());
            assert!(
                wait_for_user_message(&seen).await.is_some(),
                "the first-run seed is sent on session/new, not only on replay"
            );

            let accepted = connection
                .send_request(v2::PromptRequest::new(
                    created.session_id.clone(),
                    vec![v2::ContentBlock::Text(v2::TextContent::new("hello"))],
                ))
                .block_task()
                .await?;
            let message_id = accepted.message_id.to_string();
            assert!(!message_id.is_empty());

            let reason = wait_for_idle(&seen).await;
            assert!(
                matches!(reason, Some(v2::StopReason::EndTurn)),
                "the turn reaches a terminal idle: {reason:?}"
            );

            {
                let seen = seen.lock().unwrap();
                assert!(
                    seen.iter()
                        .any(|entry| matches!(entry, Seen::User(id) if id == &message_id)),
                    "the inserted user message echoes the accepted id"
                );
                let running = seen
                    .iter()
                    .position(|entry| matches!(entry, Seen::Running))
                    .expect("running is reported");
                let idle = seen
                    .iter()
                    .position(|entry| matches!(entry, Seen::Idle(_)))
                    .expect("the terminal idle is reported");
                assert!(
                    running < idle,
                    "running precedes the terminal idle: {seen:?}"
                );
                assert_eq!(
                    seen.iter()
                        .filter(|entry| matches!(entry, Seen::Idle(_)))
                        .count(),
                    1,
                    "exactly one terminal idle: {seen:?}"
                );
                assert!(
                    seen.iter()
                        .any(|entry| matches!(entry, Seen::AgentText(text) if text == "ok")),
                    "the model text streams as an agent_message_chunk"
                );
            }

            let listed = connection
                .send_request(v2::ListSessionsRequest::new())
                .block_task()
                .await?;
            assert!(
                listed
                    .sessions
                    .iter()
                    .any(|session| session.session_id == created.session_id),
                "the session is listed"
            );
            Ok(())
        })
        .await
        .expect("v2 lifecycle");
}

#[tokio::test(flavor = "multi_thread")]
async fn v1_and_v2_negotiate_independently() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;

    let v1_agent = spawn_agent(dir.path(), &model_config(&server.uri()));
    let v1 = Client
        .builder()
        .connect_with(v1_agent, |connection: ConnectionTo<Agent>| async move {
            let response = connection
                .send_request(
                    agentkit_tackle::agent_client_protocol::schema::v1::InitializeRequest::new(
                        ProtocolVersion::V1,
                    ),
                )
                .block_task()
                .await?;
            assert_eq!(response.protocol_version, ProtocolVersion::V1);
            Ok(())
        })
        .await;
    assert!(v1.is_ok(), "v1 negotiation");

    let v2_agent = spawn_agent(dir.path(), &model_config(&server.uri()));
    let v2 = Client
        .v2()
        .connect_with(v2_agent, |connection: V2ConnectionTo<Agent>| async move {
            let response = connection
                .send_request(v2::InitializeRequest::new(
                    ProtocolVersion::V2,
                    v2::Implementation::new("test-client", "1.0.0"),
                ))
                .block_task()
                .await?;
            assert_eq!(response.protocol_version, ProtocolVersion::V2);
            Ok(())
        })
        .await;
    assert!(v2.is_ok(), "v2 negotiation");
}


async fn wait_for_compaction(seen: &Arc<Mutex<Vec<Seen>>>) -> Option<v2::CompactionStatus> {
    for _ in 0..200 {
        if let Some(Seen::Compaction(status)) = seen
            .lock()
            .unwrap()
            .iter()
            .find(|entry| matches!(entry, Seen::Compaction(_)))
        {
            return Some(status.clone());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    None
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_fork_creates_a_distinct_session_that_can_be_prompted() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    let agent = spawn_agent(dir.path(), &model_config(&server.uri()));
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let handler = Arc::clone(&seen);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                if let v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(idle)) =
                    &notification.update
                {
                    handler
                        .lock()
                        .unwrap()
                        .push(Seen::Idle(idle.stop_reason.clone()));
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
            let cwd = std::env::current_dir().unwrap();
            let source = connection
                .send_request(v2::NewSessionRequest::new(cwd.clone()))
                .block_task()
                .await?;
            let fork = connection
                .send_request(v2::ForkSessionRequest::new(
                    source.session_id.clone(),
                    cwd.clone(),
                ))
                .block_task()
                .await?;
            assert_ne!(fork.session_id, source.session_id);
            assert!(!fork.session_id.0.is_empty());

            let accepted = connection
                .send_request(v2::PromptRequest::new(
                    fork.session_id.clone(),
                    vec![v2::ContentBlock::Text(v2::TextContent::new("hello fork"))],
                ))
                .block_task()
                .await?;
            assert!(!accepted.message_id.0.is_empty());
            assert!(
                matches!(wait_for_idle(&seen).await, Some(v2::StopReason::EndTurn)),
                "the fork can be prompted without a resume"
            );
            Ok(())
        })
        .await
        .expect("v2 fork");
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_compaction_emits_a_completed_compaction_update() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    let agent = spawn_agent(dir.path(), &model_config(&server.uri()));
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let handler = Arc::clone(&seen);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                match &notification.update {
                    v2::SessionUpdate::CompactionUpdate(update) => {
                        handler
                            .lock()
                            .unwrap()
                            .push(Seen::Compaction(update.status.clone()));
                    }
                    v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(idle)) => {
                        handler
                            .lock()
                            .unwrap()
                            .push(Seen::Idle(idle.stop_reason.clone()));
                    }
                    _ => {}
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
            let created = connection
                .send_request(v2::NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;
            connection
                .send_request(v2::PromptRequest::new(
                    created.session_id.clone(),
                    vec![v2::ContentBlock::Text(v2::TextContent::new("/compact"))],
                ))
                .block_task()
                .await?;
            assert!(
                matches!(
                    wait_for_compaction(&seen).await,
                    Some(v2::CompactionStatus::Completed)
                ),
                "compaction reports a completed update"
            );
            assert!(
                wait_for_idle(&seen).await.is_some(),
                "compaction ends with an idle"
            );

            // The compaction path must release its lease: a follow-up prompt
            // must not be rejected as session-actively-owned.
            let accepted = connection
                .send_request(v2::PromptRequest::new(
                    created.session_id,
                    vec![v2::ContentBlock::Text(v2::TextContent::new("hello again"))],
                ))
                .block_task()
                .await?;
            assert!(!accepted.message_id.0.is_empty());
            Ok(())
        })
        .await
        .expect("v2 compaction");
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_fork_prompt_is_intercepted_by_the_harness() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    let agent = spawn_agent(dir.path(), &model_config(&server.uri()));

    let texts: Arc<Mutex<Vec<String>>> = Arc::default();
    let handler = Arc::clone(&texts);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                if let v2::SessionUpdate::AgentMessage(message) = &notification.update {
                    if let agentkit_tackle::agent_client_protocol::schema::MaybeUndefined::Value(
                        blocks,
                    ) = &message.content
                    {
                        for block in blocks {
                            if let v2::ContentBlock::Text(text) = block {
                                handler.lock().unwrap().push(text.text.clone());
                            }
                        }
                    }
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
            let created = connection
                .send_request(v2::NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;
            let accepted = connection
                .send_request(v2::PromptRequest::new(
                    created.session_id.clone(),
                    vec![v2::ContentBlock::Text(v2::TextContent::new("/fork"))],
                ))
                .block_task()
                .await?;
            assert!(!accepted.message_id.0.is_empty());

            let mut intercepted = false;
            for _ in 0..200 {
                if texts
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|text| text.contains("Forked this conversation to session"))
                {
                    intercepted = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            assert!(
                intercepted,
                "the harness reports the fork, not the model: {:?}",
                texts.lock().unwrap()
            );

            let listed = connection
                .send_request(v2::ListSessionsRequest::new())
                .block_task()
                .await?;
            assert!(
                listed.sessions.len() >= 2,
                "the fork is a stored session: {listed:?}"
            );
            Ok(())
        })
        .await
        .expect("v2 /fork fallback");
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_titling_surfaces_the_session_title() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    let agent = spawn_agent(
        dir.path(),
        &model_config(&server.uri()).replace("enabled = false", "enabled = true"),
    );

    let titles: Arc<Mutex<Vec<String>>> = Arc::default();
    let handler = Arc::clone(&titles);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                if let v2::SessionUpdate::SessionInfoUpdate(update) = &notification.update {
                    if let agentkit_tackle::agent_client_protocol::schema::MaybeUndefined::Value(
                        title,
                    ) = &update.title
                    {
                        handler.lock().unwrap().push(title.clone());
                    }
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
            let created = connection
                .send_request(v2::NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;
            connection
                .send_request(v2::PromptRequest::new(
                    created.session_id,
                    vec![v2::ContentBlock::Text(v2::TextContent::new("hello"))],
                ))
                .block_task()
                .await?;

            let mut titled = false;
            for _ in 0..200 {
                if titles.lock().unwrap().iter().any(|title| title == "ok") {
                    titled = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            assert!(
                titled,
                "the titling script's title reaches the client: {:?}",
                titles.lock().unwrap()
            );
            Ok(())
        })
        .await
        .expect("v2 titling");
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_advertises_each_mcp_tool_as_a_direct_command() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    let mut config = model_config(&server.uri());
    config.push_str(&format!(
        "\n[mcp_servers.testfs]\ntransport = \"stdio\"\ncommand = {:?}\nargs = [\"--root\", {:?}]\n",
        env!("CARGO_BIN_EXE_tackle-mcp-fs"),
        dir.path(),
    ));
    let agent = spawn_agent(dir.path(), &config);

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
            let created = connection
                .send_request(v2::NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;
            let names: Vec<&str> = created
                .available_commands
                .iter()
                .map(|command| command.name.as_str())
                .collect();
            assert!(
                names.contains(&"!mcp.testfs.read_file"),
                "each MCP tool is advertised under its `!`-prefixed name: {names:?}"
            );
            Ok(())
        })
        .await
        .expect("v2 per-tool commands");
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_direct_tool_invocation_runs_without_the_model() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("README.md"), "hello from readme\n").unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    let mut config = model_config(&server.uri());
    config.push_str(&format!(
        "\n[mcp_servers.testfs]\ntransport = \"stdio\"\ncommand = {:?}\nargs = [\"--root\", {:?}]\n",
        env!("CARGO_BIN_EXE_tackle-mcp-fs"),
        dir.path(),
    ));
    let agent = spawn_agent(dir.path(), &config);

    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let handler = Arc::clone(&seen);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                match &notification.update {
                    v2::SessionUpdate::ToolCallUpdate(update) => {
                        if let agentkit_tackle::agent_client_protocol::schema::MaybeUndefined::Value(
                            content,
                        ) = &update.content
                        {
                            for item in content {
                                if let v2::ToolCallContent::Content(content) = item {
                                    if let v2::ContentBlock::Text(text) = &content.content {
                                        handler
                                            .lock()
                                            .unwrap()
                                            .push(Seen::ToolText(text.text.clone()));
                                    }
                                }
                            }
                        }
                    }
                    v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(idle)) => {
                        handler
                            .lock()
                            .unwrap()
                            .push(Seen::Idle(idle.stop_reason.clone()));
                    }
                    v2::SessionUpdate::AgentMessageChunk(chunk) => {
                        if let v2::ContentBlock::Text(text) = &chunk.content {
                            handler
                                .lock()
                                .unwrap()
                                .push(Seen::AgentText(text.text.clone()));
                        }
                    }
                    _ => {}
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
            let created = connection
                .send_request(v2::NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;
            connection
                .send_request(v2::PromptRequest::new(
                    created.session_id,
                    vec![v2::ContentBlock::Text(v2::TextContent::new(
                        "/!mcp.testfs.read_file {\"path\":\"README.md\"}",
                    ))],
                ))
                .block_task()
                .await?;

            let mut read = false;
            for _ in 0..200 {
                if seen.lock().unwrap().iter().any(
                    |entry| matches!(entry, Seen::ToolText(text) if text.contains("hello from readme")),
                ) {
                    read = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            assert!(
                read,
                "the direct tool result is rendered: {:?}",
                seen.lock().unwrap()
            );
            assert!(
                matches!(wait_for_idle(&seen).await, Some(v2::StopReason::EndTurn)),
                "the direct turn ends with idle"
            );
            assert!(
                !seen
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|entry| matches!(entry, Seen::AgentText(text) if text == "ok")),
                "the model is not invoked for a direct tool call"
            );
            Ok(())
        })
        .await
        .expect("v2 direct invocation");
}

async fn wait_for_agent_text(seen: &Arc<Mutex<Vec<Seen>>>, needle: &str) -> bool {
    for _ in 0..200 {
        if seen.lock().unwrap().iter().any(
            |entry| matches!(entry, Seen::AgentText(text) if text.contains(needle)),
        ) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_context_command_returns_a_report() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    let agent = spawn_agent(dir.path(), &model_config(&server.uri()));
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let handler = Arc::clone(&seen);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                match &notification.update {
                    v2::SessionUpdate::AgentMessage(message) => {
                        if let agentkit_tackle::agent_client_protocol::schema::MaybeUndefined::Value(
                            blocks,
                        ) = &message.content
                        {
                            for block in blocks {
                                if let v2::ContentBlock::Text(text) = block {
                                    handler
                                        .lock()
                                        .unwrap()
                                        .push(Seen::AgentText(text.text.clone()));
                                }
                            }
                        }
                    }
                    v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(idle)) => {
                        handler
                            .lock()
                            .unwrap()
                            .push(Seen::Idle(idle.stop_reason.clone()));
                    }
                    _ => {}
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
            let created = connection
                .send_request(v2::NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;
            connection
                .send_request(v2::PromptRequest::new(
                    created.session_id,
                    vec![v2::ContentBlock::Text(v2::TextContent::new("/context"))],
                ))
                .block_task()
                .await?;
            assert!(
                wait_for_agent_text(&seen, "Context window:").await,
                "the context report is rendered: {:?}",
                seen.lock().unwrap()
            );
            assert!(
                matches!(wait_for_idle(&seen).await, Some(v2::StopReason::EndTurn)),
                "the context turn ends with idle"
            );
            Ok(())
        })
        .await
        .expect("v2 /context");
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_runtime_commands_answer_without_the_model() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    let agent = spawn_agent(dir.path(), &model_config(&server.uri()));
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let handler = Arc::clone(&seen);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                match &notification.update {
                    v2::SessionUpdate::AgentMessage(message) => {
                        if let agentkit_tackle::agent_client_protocol::schema::MaybeUndefined::Value(
                            blocks,
                        ) = &message.content
                        {
                            for block in blocks {
                                if let v2::ContentBlock::Text(text) = block {
                                    handler
                                        .lock()
                                        .unwrap()
                                        .push(Seen::AgentText(text.text.clone()));
                                }
                            }
                        }
                    }
                    v2::SessionUpdate::AgentMessageChunk(chunk) => {
                        if let v2::ContentBlock::Text(text) = &chunk.content {
                            handler
                                .lock()
                                .unwrap()
                                .push(Seen::AgentText(text.text.clone()));
                        }
                    }
                    v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(idle)) => {
                        handler
                            .lock()
                            .unwrap()
                            .push(Seen::Idle(idle.stop_reason.clone()));
                    }
                    _ => {}
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
            let created = connection
                .send_request(v2::NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;
            connection
                .send_request(v2::PromptRequest::new(
                    created.session_id.clone(),
                    vec![v2::ContentBlock::Text(v2::TextContent::new("/mcp status"))],
                ))
                .block_task()
                .await?;
            assert!(
                wait_for_agent_text(&seen, "No MCP servers configured").await,
                "the mcp status reply is rendered: {:?}",
                seen.lock().unwrap()
            );
            assert!(
                matches!(wait_for_idle(&seen).await, Some(v2::StopReason::EndTurn)),
                "the mcp turn ends with idle"
            );

            // An unknown /mcp subcommand is an explicit usage reply, never
            // a fallthrough to a model turn.
            seen.lock().unwrap().clear();
            connection
                .send_request(v2::PromptRequest::new(
                    created.session_id.clone(),
                    vec![v2::ContentBlock::Text(v2::TextContent::new("/mcp bogus"))],
                ))
                .block_task()
                .await?;
            assert!(
                wait_for_agent_text(&seen, "Usage: /mcp <enable|disable|status>").await,
                "the unknown subcommand answers with usage: {:?}",
                seen.lock().unwrap()
            );
            assert!(
                !seen
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|entry| matches!(entry, Seen::AgentText(text) if text == "ok")),
                "an unknown /mcp subcommand never reaches the model"
            );

            connection
                .send_request(v2::PromptRequest::new(
                    created.session_id,
                    vec![v2::ContentBlock::Text(v2::TextContent::new(
                        "/editor fs true",
                    ))],
                ))
                .block_task()
                .await?;
            assert!(
                wait_for_agent_text(&seen, "not available under ACP v2").await,
                "the editor reply is rendered: {:?}",
                seen.lock().unwrap()
            );
            assert!(
                !seen
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|entry| matches!(entry, Seen::AgentText(text) if text == "ok")),
                "the model is not invoked for a runtime command"
            );
            Ok(())
        })
        .await
        .expect("v2 runtime commands");
}

struct SlowModel;

impl Respond for SlowModel {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        ScriptedModel.respond(request).set_delay(Duration::from_millis(150))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_concurrent_prompts_queue_instead_of_failing() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(SlowModel)
        .mount(&server)
        .await;
    let agent = spawn_agent(dir.path(), &model_config(&server.uri()));
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let handler = Arc::clone(&seen);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                if let v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(idle)) =
                    &notification.update
                {
                    handler
                        .lock()
                        .unwrap()
                        .push(Seen::Idle(idle.stop_reason.clone()));
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
            let created = connection
                .send_request(v2::NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;

            // Both prompts are accepted; the second queues behind the first
            // turn instead of returning session-actively-owned.
            let first = connection
                .send_request(v2::PromptRequest::new(
                    created.session_id.clone(),
                    vec![v2::ContentBlock::Text(v2::TextContent::new("one"))],
                ))
                .block_task()
                .await?;
            let second = connection
                .send_request(v2::PromptRequest::new(
                    created.session_id,
                    vec![v2::ContentBlock::Text(v2::TextContent::new("two"))],
                ))
                .block_task()
                .await?;
            assert!(!first.message_id.0.is_empty());
            assert!(!second.message_id.0.is_empty());

            let mut idles = 0;
            for _ in 0..400 {
                idles = seen
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|entry| matches!(entry, Seen::Idle(_)))
                    .count();
                if idles >= 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            assert_eq!(idles, 2, "both queued turns reach idle: {:?}", seen.lock().unwrap());
            Ok(())
        })
        .await
        .expect("v2 concurrent prompts");
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_fork_is_seeded_and_replays_on_resume() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    let agent = spawn_agent(dir.path(), &model_config(&server.uri()));
    let texts: Arc<Mutex<Vec<String>>> = Arc::default();
    let handler = Arc::clone(&texts);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                if let v2::SessionUpdate::UserMessage(message) = &notification.update {
                    if let agentkit_tackle::agent_client_protocol::schema::MaybeUndefined::Value(
                        blocks,
                    ) = &message.content
                    {
                        for block in blocks {
                            if let v2::ContentBlock::Text(text) = block {
                                handler.lock().unwrap().push(text.text.clone());
                            }
                        }
                    }
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
            let cwd = std::env::current_dir().unwrap();
            let source = connection
                .send_request(v2::NewSessionRequest::new(cwd.clone()))
                .block_task()
                .await?;
            let fork = connection
                .send_request(v2::ForkSessionRequest::new(
                    source.session_id,
                    cwd.clone(),
                ))
                .block_task()
                .await?;

            connection
                .send_request(
                    v2::ResumeSessionRequest::new(fork.session_id, cwd)
                        .replay_from(v2::ReplayFrom::Start(v2::ReplayFromStart::new())),
                )
                .block_task()
                .await?;

            let mut seeded = false;
            for _ in 0..200 {
                if texts
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|text| text.contains("Forked from session"))
                {
                    seeded = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            assert!(
                seeded,
                "the fork seed replays on resume: {:?}",
                texts.lock().unwrap()
            );
            Ok(())
        })
        .await
        .expect("v2 fork seed");
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_reports_the_absence_of_mcp_tooling() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    let agent = spawn_agent(dir.path(), &model_config(&server.uri()));
    let notices: Arc<Mutex<Vec<String>>> = Arc::default();
    let handler = Arc::clone(&notices);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                if let v2::SessionUpdate::Notice(notice) = &notification.update {
                    handler.lock().unwrap().push(notice.title.clone());
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
            connection
                .send_request(v2::NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;

            let mut reported = false;
            for _ in 0..200 {
                if notices
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|text| text.contains("No MCP servers configured"))
                {
                    reported = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            assert!(
                reported,
                "the missing tooling is reported: {:?}",
                notices.lock().unwrap()
            );
            Ok(())
        })
        .await
        .expect("v2 absence notice");
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_new_session_sends_a_usage_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    let agent = spawn_agent(dir.path(), &model_config(&server.uri()));
    let usages: Arc<Mutex<Vec<(u64, u64)>>> = Arc::default();
    let handler = Arc::clone(&usages);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                if let v2::SessionUpdate::UsageUpdate(update) = &notification.update {
                    handler.lock().unwrap().push((update.used, update.size));
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
            connection
                .send_request(v2::NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;

            let mut snapshot = None;
            for _ in 0..200 {
                if let Some(usage) = usages.lock().unwrap().first().copied() {
                    snapshot = Some(usage);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            let (used, size) = snapshot.expect("a usage snapshot follows session/new");
            assert!(used > 0, "the seed contributes context: {used}");
            assert!(size > 0, "the model window is known: {size}");
            Ok(())
        })
        .await
        .expect("v2 usage snapshot");
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_set_config_option_announces_the_change() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    let agent = spawn_agent(dir.path(), &model_config(&server.uri()));
    let updates: Arc<Mutex<Vec<Vec<v2::SessionConfigOption>>>> = Arc::default();
    let handler = Arc::clone(&updates);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                if let v2::SessionUpdate::ConfigOptionUpdate(update) = &notification.update {
                    handler.lock().unwrap().push(update.config_options.clone());
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
            let created = connection
                .send_request(v2::NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;
            let actor = created
                .config_options
                .iter()
                .find(|option| option.config_id.0.as_ref() == "actor")
                .expect("an actor config option");
            let (current, choices) = match &actor.kind {
                v2::SessionConfigKind::Select(select) => {
                    let v2::SessionConfigSelectOptions::Ungrouped(choices) = &select.options else {
                        panic!("expected ungrouped actor choices");
                    };
                    (select.current_value.clone(), choices.clone())
                }
                _ => panic!("expected a select actor option"),
            };
            let alternative = choices
                .iter()
                .find(|choice| choice.value != current)
                .expect("an alternative actor")
                .value
                .clone();

            let response = connection
                .send_request(v2::SetSessionConfigOptionRequest::new(
                    created.session_id,
                    "actor",
                    v2::SessionConfigOptionValue::Id {
                        value: alternative.clone(),
                    },
                ))
                .block_task()
                .await?;
            let updated = response
                .config_options
                .iter()
                .find(|option| option.config_id.0.as_ref() == "actor")
                .expect("the refreshed actor option");
            let v2::SessionConfigKind::Select(select) = &updated.kind else {
                panic!("expected a select actor option");
            };
            assert_eq!(select.current_value, alternative);

            let mut announced = false;
            for _ in 0..200 {
                if !updates.lock().unwrap().is_empty() {
                    announced = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            assert!(announced, "the config change is announced");
            Ok(())
        })
        .await
        .expect("v2 set config option");
}

#[derive(Debug, Clone, PartialEq)]
enum ReplaySeen {
    User(String),
    AgentChunk(String),
    Agent(String),
    Idle,
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_resume_replays_ordered_ids_and_rejects_unknown_cursors() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    let agent = spawn_agent(dir.path(), &model_config(&server.uri()));
    let seen: Arc<Mutex<Vec<ReplaySeen>>> = Arc::default();
    let handler = Arc::clone(&seen);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                let mut seen = handler.lock().unwrap();
                match &notification.update {
                    v2::SessionUpdate::UserMessage(message) => {
                        seen.push(ReplaySeen::User(message.message_id.to_string()));
                    }
                    v2::SessionUpdate::AgentMessageChunk(chunk) => {
                        seen.push(ReplaySeen::AgentChunk(chunk.message_id.to_string()));
                    }
                    v2::SessionUpdate::AgentMessage(message) => {
                        seen.push(ReplaySeen::Agent(message.message_id.to_string()));
                    }
                    v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(_)) => {
                        seen.push(ReplaySeen::Idle);
                    }
                    _ => {}
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
            let cwd = std::env::current_dir().unwrap();
            let created = connection
                .send_request(v2::NewSessionRequest::new(cwd.clone()))
                .block_task()
                .await?;

            let seed_id = {
                let mut found = None;
                for _ in 0..200 {
                    if let Some(ReplaySeen::User(id)) = seen.lock().unwrap().first().cloned() {
                        found = Some(id);
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                found.expect("the first-run seed is announced")
            };

            let accepted = connection
                .send_request(v2::PromptRequest::new(
                    created.session_id.clone(),
                    vec![v2::ContentBlock::Text(v2::TextContent::new("hello"))],
                ))
                .block_task()
                .await?;
            let prompt_id = accepted.message_id.to_string();
            for _ in 0..200 {
                if seen.lock().unwrap().contains(&ReplaySeen::Idle) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            let assistant_id = seen
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find_map(|entry| match entry {
                    ReplaySeen::AgentChunk(id) => Some(id.clone()),
                    _ => None,
                })
                .expect("the assistant message streams");

            seen.lock().unwrap().clear();
            connection
                .send_request(
                    v2::ResumeSessionRequest::new(created.session_id.clone(), cwd.clone())
                        .replay_from(v2::ReplayFrom::Start(v2::ReplayFromStart::new())),
                )
                .block_task()
                .await?;
            for _ in 0..200 {
                if seen.lock().unwrap().contains(&ReplaySeen::Idle) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            assert_eq!(
                *seen.lock().unwrap(),
                vec![
                    ReplaySeen::User(seed_id),
                    ReplaySeen::User(prompt_id),
                    ReplaySeen::Agent(assistant_id),
                    ReplaySeen::Idle,
                ],
                "replay preserves order, stored ids, and ends with the idle snapshot"
            );

            let error = match connection
                .send_request(
                    v2::ResumeSessionRequest::new(created.session_id, cwd).replay_from(
                        v2::ReplayFrom::Other(v2::OtherReplayFrom::new(
                            "_unsupported",
                            Default::default(),
                        )),
                    ),
                )
                .block_task()
                .await
            {
                Ok(_) => panic!("an unsupported replay cursor must be rejected"),
                Err(error) => error,
            };
            assert_eq!(
                i32::from(error.code),
                -32602,
                "unsupported cursors are invalid params: {error:?}"
            );
            Ok(())
        })
        .await
        .expect("v2 resume replay");
}


#[tokio::test(flavor = "multi_thread")]
async fn v2_close_and_delete_cancel_in_flight_turns() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(HangingModel)
        .mount(&server)
        .await;
    let agent = spawn_agent(dir.path(), &model_config(&server.uri()));
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let handler = Arc::clone(&seen);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                if let v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(idle)) =
                    &notification.update
                {
                    handler
                        .lock()
                        .unwrap()
                        .push(Seen::Idle(idle.stop_reason.clone()));
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
            let cwd = std::env::current_dir().unwrap();

            // Close during a turn: cancelled idle, session still listable.
            let closed = connection
                .send_request(v2::NewSessionRequest::new(cwd.clone()))
                .block_task()
                .await?;
            let prompt = connection.send_request(v2::PromptRequest::new(
                closed.session_id.clone(),
                vec![v2::ContentBlock::Text(v2::TextContent::new("hello"))],
            ));
            connection
                .send_request(v2::CloseSessionRequest::new(closed.session_id.clone()))
                .block_task()
                .await?;
            prompt.block_task().await?;
            let idles = wait_for_idles(&seen, 1).await;
            assert!(
                matches!(idles.first(), Some(Seen::Idle(Some(v2::StopReason::Cancelled)))),
                "close cancels the turn: {idles:?}"
            );
            let listed = connection
                .send_request(v2::ListSessionsRequest::new())
                .block_task()
                .await?;
            assert!(
                listed
                    .sessions
                    .iter()
                    .any(|session| session.session_id == closed.session_id),
                "close keeps the session listable"
            );

            // Delete during a turn: cancelled idle, then not listable.
            let deleted = connection
                .send_request(v2::NewSessionRequest::new(cwd))
                .block_task()
                .await?;
            let prompt = connection.send_request(v2::PromptRequest::new(
                deleted.session_id.clone(),
                vec![v2::ContentBlock::Text(v2::TextContent::new("hello"))],
            ));
            connection
                .send_request(v2::DeleteSessionRequest::new(deleted.session_id.clone()))
                .block_task()
                .await?;
            prompt.block_task().await?;
            let idles = wait_for_idles(&seen, 2).await;
            assert!(
                matches!(idles.get(1), Some(Seen::Idle(Some(v2::StopReason::Cancelled)))),
                "delete cancels the turn: {idles:?}"
            );
            let listed = connection
                .send_request(v2::ListSessionsRequest::new())
                .block_task()
                .await?;
            assert!(
                !listed
                    .sessions
                    .iter()
                    .any(|session| session.session_id == deleted.session_id),
                "delete removes the session from the list"
            );
            Ok(())
        })
        .await
        .expect("v2 close/delete");
}

async fn wait_for_idles(
    seen: &Arc<Mutex<Vec<Seen>>>,
    count: usize,
) -> Vec<Seen> {
    for _ in 0..400 {
        {
            let seen = seen.lock().unwrap();
            let idles: Vec<Seen> = seen
                .iter()
                .filter(|entry| matches!(entry, Seen::Idle(_)))
                .cloned()
                .collect();
            if idles.len() >= count {
                return idles;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    seen.lock().unwrap().clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_initialize_advertises_capabilities_and_additional_directories() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    let agent = spawn_agent(dir.path(), &model_config(&server.uri()));

    Client
        .v2()
        .connect_with(agent, |connection: V2ConnectionTo<Agent>| async move {
            let init = connection
                .send_request(v2::InitializeRequest::new(
                    ProtocolVersion::V2,
                    v2::Implementation::new("test-client", "1.0.0"),
                ))
                .block_task()
                .await?;
            let session = init
                .capabilities
                .session
                .expect("the v2 session capability map");
            assert!(
                session
                    .prompt
                    .as_ref()
                    .and_then(|prompt| prompt.image.as_ref())
                    .is_some(),
                "image prompts are advertised"
            );
            assert!(
                session
                    .prompt
                    .as_ref()
                    .and_then(|prompt| prompt.embedded_context.as_ref())
                    .is_some(),
                "embedded context is advertised"
            );
            let mcp = session.mcp.as_ref().expect("MCP capabilities");
            assert!(mcp.stdio.is_some(), "stdio MCP");
            assert!(mcp.http.is_some(), "HTTP MCP");
            assert!(session.delete.is_some(), "session/delete");
            assert!(
                session.additional_directories.is_some(),
                "additionalDirectories"
            );
            assert!(session.fork.is_some(), "session/fork");

            let extra = std::path::PathBuf::from("/tmp/tackle-extra-root");
            let created = connection
                .send_request(
                    v2::NewSessionRequest::new(std::env::current_dir().unwrap())
                        .additional_directories(vec![extra.clone()]),
                )
                .block_task()
                .await?;
            let listed = connection
                .send_request(v2::ListSessionsRequest::new())
                .block_task()
                .await?;
            let info = listed
                .sessions
                .iter()
                .find(|info| info.session_id == created.session_id)
                .expect("the created session is listed");
            assert_eq!(
                info.additional_directories,
                vec![v2::AbsolutePath::new(extra)],
                "additional directories round-trip through session/list"
            );
            Ok(())
        })
        .await
        .expect("v2 capabilities and additional directories");
}


#[tokio::test(flavor = "multi_thread")]
async fn v2_failure_after_insertion_keeps_the_message_and_reports_the_error() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(EmptyModels)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(EmptyModels)
        .mount(&server)
        .await;
    let config = format!(
        "[endpoints.mock]\nbase_url = \"{}\"\nwire_format = \"openai-chat-completions\"\nauth = \"none\"\nmodels = []\n\n\
         [scripts.titling]\nevents = [\"title_trigger\"]\nfile = \"builtin:titling.rhai\"\nenabled = false\n",
        server.uri()
    );
    let agent = spawn_agent(dir.path(), &config);
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let handler = Arc::clone(&seen);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                let mut seen = handler.lock().unwrap();
                match &notification.update {
                    v2::SessionUpdate::UserMessage(message) => {
                        seen.push(Seen::User(message.message_id.to_string()));
                    }
                    v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(idle)) => {
                        seen.push(Seen::Idle(idle.stop_reason.clone()));
                    }
                    _ => {}
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
            let created = connection
                .send_request(v2::NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;
            let accepted = connection
                .send_request(v2::PromptRequest::new(
                    created.session_id,
                    vec![v2::ContentBlock::Text(v2::TextContent::new("hello"))],
                ))
                .block_task()
                .await?;
            let message_id = accepted.message_id.to_string();
            assert!(!message_id.is_empty());

            let reason = wait_for_idle(&seen).await.expect("the turn reaches idle");
            let v2::StopReason::Error(error) = reason else {
                panic!("the failed turn reports error: {reason:?}");
            };
            assert!(
                error.error.is_some(),
                "the failure detail rides the stop reason: {error:?}"
            );
            assert!(
                seen.lock().unwrap().iter().any(
                    |entry| matches!(entry, Seen::User(id) if id == &message_id)
                ),
                "the inserted user message keeps its id: {:?}",
                seen.lock().unwrap()
            );
            Ok(())
        })
        .await
        .expect("v2 failure after insertion");
}

/// A model that asks for the echo fixture's elicitation tool, then answers
/// once the tool result is in the transcript.
struct ElicitModel;

impl Respond for ElicitModel {
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
        let arguments = serde_json::to_string(&serde_json::json!({})).unwrap();
        if has_tool_result {
            let lines = [
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{"role":"assistant"}}]}"#,
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{"content":"done"}}]}"#,
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
                "data: [DONE]",
            ];
            ResponseTemplate::new(200)
                .set_body_raw(lines.join("\n\n") + "\n", "text/event-stream")
        } else {
            let lines = [
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{"role":"assistant"}}]}"#.to_owned(),
                format!(
                    r#"data: {{"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"call_elicit","type":"function","function":{{"name":"mcp.echo.elicit","arguments":{arguments}}}}}]}}}}]}}"#
                ),
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#.to_owned(),
                "data: [DONE]".to_owned(),
            ];
            ResponseTemplate::new(200)
                .set_body_raw(lines.join("\n\n") + "\n", "text/event-stream")
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_elicitation_reaches_the_client_and_returns_to_the_tool() {
    use agentkit_tackle::agent_client_protocol::on_receive_request;

    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ElicitModel)
        .mount(&server)
        .await;
    let config = format!(
        "[endpoints.mock]\nbase_url = \"{}\"\nwire_format = \"openai-chat-completions\"\nauth = \"none\"\nmodels = [\"gpt-4o-mini\"]\n\n\
         [defaults]\nmodel = \"mock/gpt-4o-mini\"\n\n\
         [mcp_servers.echo]\ntransport = \"stdio\"\ncommand = {}\n\n\
         [scripts.titling]\nevents = [\"title_turn\"]\nfile = \"builtin:titling.rhai\"\nenabled = false\n",
        server.uri(),
        serde_json::to_string(env!("CARGO_BIN_EXE_tackle-mcp-echo")).unwrap()
    );
    let agent = spawn_agent(dir.path(), &config);
    let elicitations: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let elicitations_handler = Arc::clone(&elicitations);
    let seen_handler = Arc::clone(&seen);

    Client
        .v2()
        .on_receive_notification(
            async move |notification: v2::UpdateSessionNotification, _cx| {
                let mut seen = seen_handler.lock().unwrap();
                match &notification.update {
                    v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(idle)) => {
                        seen.push(Seen::Idle(idle.stop_reason.clone()));
                    }
                    v2::SessionUpdate::AgentMessage(message) => {
                        if let agentkit_tackle::agent_client_protocol::schema::MaybeUndefined::Value(
                            blocks,
                        ) = &message.content
                        {
                            for block in blocks {
                                if let v2::ContentBlock::Text(text) = block {
                                    seen.push(Seen::AgentText(text.text.clone()));
                                }
                            }
                        }
                    }
                    v2::SessionUpdate::AgentMessageChunk(chunk) => {
                        if let v2::ContentBlock::Text(text) = &chunk.content {
                            seen.push(Seen::AgentText(text.text.clone()));
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
        .on_receive_request(
            async move |request: v2::CreateElicitationRequest, responder, _cx| {
                elicitations_handler
                    .lock()
                    .unwrap()
                    .push(request.message.clone());
                responder.respond(v2::CreateElicitationResponse::new(
                    v2::ElicitationAction::Accept(v2::ElicitationAcceptAction::new()),
                ))
            },
            on_receive_request!(),
        )
        .connect_with(agent, |connection: V2ConnectionTo<Agent>| async move {
            connection
                .send_request(
                    v2::InitializeRequest::new(
                        ProtocolVersion::V2,
                        v2::Implementation::new("test-client", "1.0.0"),
                    )
                    .capabilities(
                        v2::ClientCapabilities::new().elicitation(
                            v2::ElicitationCapabilities::new()
                                .form(v2::ElicitationFormCapabilities::default()),
                        ),
                    ),
                )
                .block_task()
                .await?;
            let created = connection
                .send_request(v2::NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;
            connection
                .send_request(v2::PromptRequest::new(
                    created.session_id,
                    vec![v2::ContentBlock::Text(v2::TextContent::new("elicit"))],
                ))
                .block_task()
                .await?;
            assert!(
                wait_for_idle(&seen).await.is_some(),
                "the tool turn reaches idle: {:?}",
                seen.lock().unwrap()
            );
            assert_eq!(
                elicitations.lock().unwrap().as_slice(),
                ["Name the project".to_owned()],
                "the MCP elicitation reaches the ACP client"
            );
            assert!(
                seen.lock()
                    .unwrap()
                    .iter()
                    .any(|entry| matches!(entry, Seen::AgentText(text) if text == "done")),
                "the tool result returns to the model: {:?}",
                seen.lock().unwrap()
            );
            Ok(())
        })
        .await
        .expect("v2 elicitation");
}
