//! Session configuration option tests (T-031): selector population with
//! discovery and fallback, switch semantics, window validation, and the
//! config_option_update.

use agentkit_tackle::agent_client_protocol::schema::v1::{
    ForkSessionRequest, InitializeRequest, LoadSessionRequest, NewSessionRequest,
    ResumeSessionRequest, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
};
use agentkit_tackle::agent_client_protocol::schema::ProtocolVersion;
use agentkit_tackle::agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo};
use agentkit_tackle::store::SessionStore;
use std::sync::{Arc as StdArc, Mutex as StdMutex};

fn spawn_agent_with_config(dir: &std::path::Path, config: &str) -> AcpAgent {
    let db_path = dir.join("sessions.db");
    let cfg = dir.join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("config.toml"), config).unwrap();
    AcpAgent::from_args([
        env!("CARGO_BIN_EXE_agentkit-tackle"),
        "stdio",
        "--config-dir",
        cfg.to_str().unwrap(),
        "--db-path",
        db_path.to_str().unwrap(),
    ])
    .unwrap()
}

/// Discovery is primary: the discoverable endpoint's models list
/// endpoint-qualified; the dead endpoint falls back to its static list.
#[tokio::test(flavor = "multi_thread")]
async fn selectors_populate_with_discovery_and_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("sessions.db");

    // The discoverable endpoint: the OpenAI list shape.
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/models"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "discovered-1"}, {"id": "discovered-2"}]
            })),
        )
        .mount(&server)
        .await;

    let cfg = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("config.toml"),
        format!(
            "[endpoints.live]\nbase_url = \"{}\"\nwire_format = \"openai-chat-completions\"\nauth = \"none\"\n\n\
             [endpoints.dead]\nbase_url = \"http://127.0.0.1:1\"\nwire_format = \"openai-chat-completions\"\nauth = \"none\"\nmodels = [\"fallback-a\", \"fallback-b\"]\n",
            server.uri()
        ),
    )
    .unwrap();
    let agent = AcpAgent::from_args([
        env!("CARGO_BIN_EXE_agentkit-tackle"),
        "stdio",
        "--config-dir",
        cfg.to_str().unwrap(),
        "--db-path",
        db_path.to_str().unwrap(),
    ])
    .unwrap();

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

            let options = created.config_options.expect("selectors populated");
            let model_selector = options
                .iter()
                .find(|option| option.id.0.as_ref() == "model")
                .expect("model selector");
            let agentkit_tackle::agent_client_protocol::schema::v1::SessionConfigKind::Select(
                select,
            ) = &model_selector.kind
            else {
                panic!("the model selector is a select");
            };
            let values: Vec<String> = match &select.options {
                agentkit_tackle::agent_client_protocol::schema::v1::SessionConfigSelectOptions::Ungrouped(
                    options,
                ) => options
                    .iter()
                    .map(|option| option.value.0.to_string())
                    .collect(),
                _ => Vec::new(),
            };
            // Discovery (endpoint-qualified) and the static fallback.
            assert!(values.contains(&"live/discovered-1".to_owned()), "{values:?}");
            assert!(values.contains(&"live/discovered-2".to_owned()), "{values:?}");
            assert!(values.contains(&"dead/fallback-a".to_owned()), "{values:?}");
            Ok(())
        })
        .await
        .expect("selector population");
}

#[tokio::test(flavor = "multi_thread")]
async fn model_switch_is_effective_the_following_turn() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("sessions.db");
    let cfg = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("config.toml"),
        "[endpoints.primary]\nbase_url = \"http://127.0.0.1:1\"\nwire_format = \"openai-chat-completions\"\nauth = \"none\"\nmodels = [\"primary-a\", \"primary-b\"]\n\n\
         [defaults]\nmodel = \"primary/primary-a\"\n",
    )
    .unwrap();
    let agent = AcpAgent::from_args([
        env!("CARGO_BIN_EXE_agentkit-tackle"),
        "stdio",
        "--config-dir",
        cfg.to_str().unwrap(),
        "--db-path",
        db_path.to_str().unwrap(),
    ])
    .unwrap();

    let config_updates: StdArc<StdMutex<usize>> = StdArc::default();
    let config_handler = config_updates.clone();

    Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                if let SessionUpdate::ConfigOptionUpdate(_) = &notification.update {
                    *config_handler.lock().unwrap() += 1;
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

            // Switch the model: a config_option_update carries the
            // refreshed selectors, and the switch is effective the
            // following turn (session metadata).
            connection
                .send_request(SetSessionConfigOptionRequest::new(
                    created.session_id.clone(),
                    "model",
                    "primary/primary-b",
                ))
                .block_task()
                .await?;
            assert!(
                *config_updates.lock().unwrap() >= 1,
                "a config_option_update followed the switch"
            );

            let db = SessionStore::connect_sqlite(&db_path).await.unwrap();
            let session = db
                .get_session(&created.session_id.to_string().replace("sess_", ""))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                agentkit_tackle::store::session_model_free(&session).as_deref(),
                Some("primary/primary-b"),
                "effective the following turn"
            );

            // An unknown actor is refused precisely.
            let err = connection
                .send_request(SetSessionConfigOptionRequest::new(
                    created.session_id.clone(),
                    "actor",
                    "no-such-actor",
                ))
                .block_task()
                .await;
            assert!(err.is_err(), "unknown actor refused");
            Ok(())
        })
        .await
        .expect("model switch round-trip");
}

#[test]
fn window_validation_and_the_context_note() {
    use agentkit_tackle::acp::config_options::context_window;

    // The bundled snapshot's windows back the validation.
    assert_eq!(context_window("models/MiniMax-M2"), Some(204_800));
    assert_eq!(context_window("models/no-such-model"), None);
}

/// Session/new seeds the effective model so the selector's current and
/// the prompt's resolution agree without an explicit client switch —
/// even with no `[defaults].model`.
#[tokio::test(flavor = "multi_thread")]
async fn session_new_seeds_the_effective_model_into_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("sessions.db");
    let agent = spawn_agent_with_config(
        dir.path(),
        "[endpoints.primary]\nbase_url = \"http://127.0.0.1:1\"\nwire_format = \"openai-chat-completions\"\nauth = \"none\"\nmodels = [\"primary-a\", \"primary-b\"]\n",
    );

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

            let options = created.config_options.expect("selectors populated");
            let model_selector = options
                .iter()
                .find(|option| option.id.0.as_ref() == "model")
                .expect("model selector");
            let agentkit_tackle::agent_client_protocol::schema::v1::SessionConfigKind::Select(
                select,
            ) = &model_selector.kind
            else {
                panic!("the model selector is a select");
            };
            let current = select.current_value.0.to_string();

            let db = SessionStore::connect_sqlite(&db_path).await.unwrap();
            let session = db
                .get_session(&created.session_id.to_string().replace("sess_", ""))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                agentkit_tackle::store::session_model_free(&session).as_deref(),
                Some(current.as_str()),
                "metadata seeds the selector's current"
            );
            assert_eq!(current, "primary/primary-a", "first listed option");
            Ok(())
        })
        .await
        .expect("seed the effective model");
}

/// Load and resume both carry selectors so a reopened session can
/// switch models before the next prompt; fork does the same for the
/// new session.
#[tokio::test(flavor = "multi_thread")]
async fn load_resume_and_fork_return_config_options() {
    let dir = tempfile::tempdir().unwrap();
    let agent = spawn_agent_with_config(
        dir.path(),
        "[endpoints.primary]\nbase_url = \"http://127.0.0.1:1\"\nwire_format = \"openai-chat-completions\"\nauth = \"none\"\nmodels = [\"primary-a\"]\n",
    );

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

            let loaded = connection
                .send_request(LoadSessionRequest::new(
                    created.session_id.clone(),
                    std::path::PathBuf::from("/work"),
                ))
                .block_task()
                .await?;
            let load_options = loaded.config_options.expect("load carries selectors");
            assert!(
                load_options
                    .iter()
                    .any(|option| option.id.0.as_ref() == "model"),
                "load has the model selector"
            );

            let resumed = connection
                .send_request(ResumeSessionRequest::new(
                    created.session_id.clone(),
                    std::path::PathBuf::from("/work"),
                ))
                .block_task()
                .await?;
            let resume_options = resumed.config_options.expect("resume carries selectors");
            assert!(
                resume_options
                    .iter()
                    .any(|option| option.id.0.as_ref() == "model"),
                "resume has the model selector"
            );

            let forked = connection
                .send_request(ForkSessionRequest::new(
                    created.session_id.clone(),
                    std::path::PathBuf::from("/work"),
                ))
                .block_task()
                .await?;
            let fork_options = forked.config_options.expect("fork carries selectors");
            assert!(
                fork_options
                    .iter()
                    .any(|option| option.id.0.as_ref() == "model"),
                "fork has the model selector"
            );
            Ok(())
        })
        .await
        .expect("selectors on load/resume/fork");
}
