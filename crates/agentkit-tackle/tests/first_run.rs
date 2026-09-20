//! First-run integration tests (T-030): empty configuration succeeds
//! via built-in defaults, the first-session seed documents the
//! configuration and syntaxes, overrides surface, missing credentials
//! advertise authMethods, and configuration errors name the file and
//! key.

use agentkit_tackle::agent_client_protocol::schema::v1::{
    AuthenticateRequest, ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest,
    TextContent,
};
use agentkit_tackle::agent_client_protocol::schema::ProtocolVersion;
use agentkit_tackle::agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo};
use agentkit_tackle::store::{SessionStore, TurnKind};

fn spawn_agent(dir: &std::path::Path) -> AcpAgent {
    AcpAgent::from_args([
        env!("CARGO_BIN_EXE_agentkit-tackle"),
        "--config-dir",
        dir.join("cfg").to_str().unwrap(),
        "--db-path",
        dir.join("sessions.db").to_str().unwrap(),
    ])
    .expect("agent arguments")
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
            let text = agentkit_tackle::acp::prompt_text_of(&seed.message.content);
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
    // An override of the built-in default actor.
    std::fs::create_dir_all(cfg.join("actors")).unwrap();
    std::fs::write(
        cfg.join("actors").join("default.md"),
        "+++\npersona = \"default\"\n+++\nCustom default actor.\n",
    )
    .unwrap();
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
            let text = agentkit_tackle::acp::prompt_text_of(&seed.message.content);
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
