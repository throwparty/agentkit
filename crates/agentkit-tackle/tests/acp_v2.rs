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
                assert!(
                    seen.iter().any(|entry| matches!(entry, Seen::Running)),
                    "running is reported before the idle"
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
