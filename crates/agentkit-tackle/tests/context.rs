//! `/context`: the ephemeral composition report, its cleanup, and its
//! no-model error path.

use agentkit_tackle::agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, ListSessionsRequest, LoadSessionRequest, NewSessionRequest,
    PromptRequest, SessionNotification, SessionUpdate, StopReason, TextContent,
};
use agentkit_tackle::agent_client_protocol::schema::ProtocolVersion;
use agentkit_tackle::agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo};
use std::sync::{Arc as StdArc, Mutex as StdMutex};

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

fn chunk_text(block: &agentkit_tackle::agent_client_protocol::schema::v1::ContentBlock) -> String {
    match block {
        agentkit_tackle::agent_client_protocol::schema::v1::ContentBlock::Text(text) => {
            text.text.clone()
        }
        _ => String::new(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn context_reports_and_leaves_no_turn_behind() {
    let dir = tempfile::tempdir().unwrap();
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/models"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "MiniMax-M2"}]
            })),
        )
        .mount(&server)
        .await;
    let agent = spawn_agent(
        dir.path(),
        &format!(
            "[endpoints.live]\nbase_url = \"{}\"\nwire_format = \"openai-chat-completions\"\nauth = \"none\"\nmodels = [\"MiniMax-M2\"]\n\n\
             [defaults]\nmodel = \"live/MiniMax-M2\"\n",
            server.uri()
        ),
    );

    let chunks: StdArc<StdMutex<Vec<String>>> = StdArc::default();
    let handler = chunks.clone();
    Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                if let SessionUpdate::AgentMessageChunk(chunk) = &notification.update {
                    handler.lock().unwrap().push(chunk_text(&chunk.content));
                }
                Ok(())
            },
            agentkit_tackle::agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(agent, move |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let created = connection
                .send_request(NewSessionRequest::new(std::path::PathBuf::from("/work")))
                .block_task()
                .await?;
            // session/new discovers models; the baseline is after that.
            connection
                .send_request(ListSessionsRequest::new())
                .block_task()
                .await?;
            let baseline = server.received_requests().await.unwrap_or_default().len();

            let response = connection
                .send_request(PromptRequest::new(
                    created.session_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("/context"))],
                ))
                .block_task()
                .await?;
            assert_eq!(response.stop_reason, StopReason::EndTurn);
            let after = server.received_requests().await.unwrap_or_default().len();
            assert_eq!(after, baseline, "no model request for /context");
            let report = chunks.lock().unwrap().join("");
            assert!(report.contains("Context window:"), "{report}");
            assert!(report.contains("Model: live/MiniMax-M2"), "{report}");
            assert!(report.contains("System prompt"), "{report}");
            assert!(report.contains("Session totals:"), "{report}");

            chunks.lock().unwrap().clear();
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
            let replayed = chunks.lock().unwrap().join("");
            assert!(!replayed.contains("/context"), "{replayed}");
            assert!(!replayed.contains("Context window:"), "{replayed}");
            Ok(())
        })
        .await
        .expect("context report");
}

#[tokio::test(flavor = "multi_thread")]
async fn context_without_a_model_errors_after_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let agent = spawn_agent(dir.path(), "");
    let chunks: StdArc<StdMutex<Vec<String>>> = StdArc::default();
    let handler = chunks.clone();
    Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                if let SessionUpdate::AgentMessageChunk(chunk) = &notification.update {
                    handler.lock().unwrap().push(chunk_text(&chunk.content));
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
            let error = connection
                .send_request(PromptRequest::new(
                    created.session_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("/context"))],
                ))
                .block_task()
                .await
                .expect_err("no model configured");
            assert!(
                format!("{error:?}")
                    .contains("no model configured; set [defaults].model or select a model"),
                "{error:?}"
            );

            chunks.lock().unwrap().clear();
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
            let replayed = chunks.lock().unwrap().join("");
            assert!(!replayed.contains("/context"), "{replayed}");
            assert!(!replayed.contains("Context window:"), "{replayed}");
            Ok(())
        })
        .await
        .expect("no-model error and cleanup");
}
