//! First-run integration tests: empty configuration succeeds
//! via built-in defaults, the first-session seed documents the
//! configuration and syntaxes, overrides surface, missing credentials
//! advertise authMethods, and configuration errors name the file and
//! key.

use agentkit_tackle::agent_client_protocol::schema::v1::{
    AuthenticateRequest, ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest,
    TextContent,
};
use agentkit_tackle::agent_client_protocol::schema::ProtocolVersion;
use agentkit_tackle::agent_client_protocol::schema::v2;
use agentkit_tackle::agent_client_protocol::{
    AcpAgent, Agent, Client, ConnectionTo, V2ConnectionTo,
};
use agentkit_tackle::store::{SessionStore, TurnKind};

fn spawn_agent(dir: &std::path::Path) -> AcpAgent {
    let command = format!(
        "cd {} && exec {} stdio --config-dir {} --db-path {}",
        dir.display(),
        env!("CARGO_BIN_EXE_agentkit-tackle"),
        dir.join("cfg").display(),
        dir.join("sessions.db").display()
    );
    AcpAgent::from_args(["/bin/sh", "-c", &command]).expect("agent arguments")
}

#[tokio::test(flavor = "multi_thread")]
async fn first_run_with_empty_config_seeds_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("sessions.db");
    let agent = spawn_agent(dir.path());

    Client
        .builder()
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            let response = connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            // No credentials configured: nothing missing to flag.
            assert!(response.auth_methods.is_empty());

            let created = connection
                .send_request(NewSessionRequest::new(std::path::PathBuf::from("/work")))
                .block_task()
                .await?;

            // The first-session seed: the loaded layers (none — the
            // built-in defaults carried it), counts, and the syntaxes.
            let db = SessionStore::connect_sqlite(&db_path).await.unwrap();
            let session_id = created.session_id.to_string().replace("sess_", "");
            let assembled = db.assemble_context(&session_id).await.unwrap();
            let seed = assembled
                .iter()
                .find(|item| item.turn_kind == TurnKind::Seed)
                .expect("first-session seed");
            let text = agentkit_tackle::content::prompt_text_of(&seed.message.content);
            assert!(text.contains("built-in defaults"), "{text}");
            assert!(text.contains("persona(s)"), "{text}");
            assert!(text.contains("/!mcp.<server>.<tool>"), "{text}");
            assert!(text.contains("/name"), "{text}");

            // The built-in asset inventory: the /compact prompt exists —
            // the interception announces with end_turn.
            let response = connection
                .send_request(PromptRequest::new(
                    created.session_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("/compact".to_owned()))],
                ))
                .block_task()
                .await?;
            assert!(matches!(
                response.stop_reason,
                agentkit_tackle::agent_client_protocol::schema::v1::StopReason::EndTurn
            ));
            Ok(())
        })
        .await
        .expect("first run with empty config");
}

#[tokio::test(flavor = "multi_thread")]
async fn builtin_overrides_surface_in_the_seed() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("sessions.db");
    let cfg = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    // An override of the built-in default actor, in the repository tree.
    let project = dir.path().join(".agentkit").join("tackle");
    std::fs::create_dir_all(project.join("actors")).unwrap();
    let actor = "+++\npersona = \"default\"\n+++\nCustom default actor.\n";
    std::fs::write(project.join("actors").join("default.md"), actor).unwrap();
    let agent = spawn_agent(dir.path());

    Client
        .builder()
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let created = connection
                .send_request(NewSessionRequest::new(std::path::PathBuf::from("/work")))
                .block_task()
                .await?;

            let db = SessionStore::connect_sqlite(&db_path).await.unwrap();
            let session_id = created.session_id.to_string().replace("sess_", "");
            let assembled = db.assemble_context(&session_id).await.unwrap();
            let seed = assembled
                .iter()
                .find(|item| item.turn_kind == TurnKind::Seed)
                .expect("seed");
            let text = agentkit_tackle::content::prompt_text_of(&seed.message.content);
            assert!(
                text.contains("actor:default"),
                "the override surfaces: {text}"
            );
            // The custom actor body is in effect (persona body passes
            // through; the override replaced the built-in wholesale).
            Ok(())
        })
        .await
        .expect("override surfacing");
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_credentials_advertise_auth_methods() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("config.toml"),
        "credential_helper = \"definitely-missing-helper\"\n\n\
         [endpoints.primary]\nbase_url = \"http://localhost:1\"\nwire_format = \"openai-chat-completions\"\nauth = \"helper\"\n",
    )
    .unwrap();
    let agent = spawn_agent(dir.path());

    Client
        .builder()
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            let response = connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            assert!(
                !response.auth_methods.is_empty(),
                "missing credentials advertise authMethods"
            );
            assert_eq!(response.auth_methods[0].id().0.as_ref(), "credentials");

            // The ACP authenticate method responds (the resolution is
            // re-attempted by the next prompt).
            connection
                .send_request(AuthenticateRequest::new("credentials"))
                .block_task()
                .await?;
            Ok(())
        })
        .await
        .expect("auth flow on missing credentials");
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_initialize_advertises_capabilities_and_auth_responds() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("config.toml"),
        "credential_helper = \"definitely-missing-helper\"\n\n\
         [endpoints.primary]\nbase_url = \"http://localhost:1\"\nwire_format = \"openai-chat-completions\"\nauth = \"helper\"\n",
    )
    .unwrap();
    let agent = spawn_agent(dir.path());

    Client
        .v2()
        .connect_with(agent, |connection: V2ConnectionTo<Agent>| async move {
            let response = connection
                .send_request(v2::InitializeRequest::new(
                    ProtocolVersion::V2,
                    v2::Implementation::new("test-client", "1.0.0"),
                ))
                .block_task()
                .await?;
            assert_eq!(response.protocol_version, ProtocolVersion::V2);
            assert_eq!(response.info.name, "tackle");
            let session = response
                .capabilities
                .session
                .expect("v2 advertises the session baseline");
            assert!(
                session.fork.is_some(),
                "the draft fork marker is advertised"
            );
            assert!(
                !response.auth_methods.is_empty(),
                "missing credentials advertise authMethods"
            );

            connection
                .send_request(v2::LoginAuthRequest::new("credentials"))
                .block_task()
                .await?;
            connection
                .send_request(v2::LogoutAuthRequest::new())
                .block_task()
                .await?;
            Ok(())
        })
        .await
        .expect("v2 initialize and auth");
}

#[test]
fn configuration_errors_name_the_file_and_key() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".agentkit/tackle")).unwrap();
    std::fs::write(
        dir.path().join(".agentkit/tackle/config.toml"),
        "credential_helper = \"rogue\"\n",
    )
    .unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agentkit-tackle"))
        .current_dir(dir.path())
        .args([
            "stdio",
            "--config-dir",
            dir.path().join("cfg").to_str().unwrap(),
            "--db-path",
            dir.path().join("sessions.db").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "the process refuses a rogue project layer"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("config.toml"), "{stderr}");
    assert!(stderr.contains("credential_helper"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_v1_session_announces_the_seed() {
    let dir = tempfile::tempdir().unwrap();
    let agent = spawn_agent(dir.path());
    let seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> = std::sync::Arc::default();
    let handler = std::sync::Arc::clone(&seen);

    Client
        .builder()
        .on_receive_notification(
            async move |notification: agentkit_tackle::agent_client_protocol::schema::v1::SessionNotification,
                        _cx| {
                if let agentkit_tackle::agent_client_protocol::schema::v1::SessionUpdate::UserMessageChunk(
                    chunk,
                ) = &notification.update
                {
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
            connection
                .send_request(NewSessionRequest::new(std::path::PathBuf::from("/work")))
                .block_task()
                .await?;

            let mut seeded = false;
            for _ in 0..200 {
                if seen
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|text| text.contains("built-in defaults"))
                {
                    seeded = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            assert!(
                seeded,
                "the seed is announced on session/new: {:?}",
                seen.lock().unwrap()
            );
            Ok(())
        })
        .await
        .expect("v1 seed announcement");
}
