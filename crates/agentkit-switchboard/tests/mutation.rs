use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use agentkit_switchboard::auth::{AuthConfig, AuthType};
use agentkit_switchboard::config::{
    ApiSurface, BillingModel, PricingConfig, ProviderConfig, SwitchboardConfig,
};
use agentkit_switchboard::models::db::ModelDb;
use agentkit_switchboard::provider::registry::ProviderRegistry;
use agentkit_switchboard::proxy::mutation::{MutationContext, MutationSet};
use agentkit_switchboard::server::routes;
use agentkit_switchboard::session::sqlite::SqliteSessionManager;
use serde_json::json;
use sqlx::SqlitePool;

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("mutations")
        .join(name)
}

fn config(mutations: Vec<std::path::PathBuf>) -> SwitchboardConfig {
    let mut providers = HashMap::new();
    providers.insert(
        "mock_openai".to_string(),
        ProviderConfig {
            identity: "mock_openai".to_string(),
            api_surface: ApiSurface::OpenaiChatCompletions,
            base_url: String::new(),
            billing: BillingModel::PayAsYouGo,
            auth: AuthConfig {
                r#type: AuthType::None,
                oauth: None,
            },
            pricing: PricingConfig {
                input_per_mtok: 0.0,
                output_per_mtok: 0.0,
                cache_read_per_mtok: None,
                cache_write_per_mtok: None,
                reasoning_per_mtok: None,
                models: HashMap::new(),
            },
            models: Some(vec!["gpt-4o".to_string()]),
            mutations,
        },
    );
    SwitchboardConfig {
        models: HashMap::new(),
        providers,
        credential_helper: None,
        session_db_path: None,
    }
}

fn compiled(config: &SwitchboardConfig) -> HashMap<String, Arc<MutationSet>> {
    let engine = Arc::new(agentkit_script::ScriptEngine::new());
    agentkit_switchboard::proxy::mutation::compile_provider_mutations(&engine, &config.providers)
        .expect("fixture scripts should compile")
}

async fn state(mock_base_url: &str, mutations: Vec<std::path::PathBuf>) -> Arc<routes::AppState> {
    let mut config = config(mutations);
    config.providers.get_mut("mock_openai").unwrap().base_url = mock_base_url.to_string();
    let mutations = compiled(&config);

    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!("src/db/migrations")
        .run(&pool)
        .await
        .unwrap();
    let registry = ProviderRegistry::new(&config.providers, "none")
        .expect("AuthType::None providers always resolve a credential");
    let model_db = ModelDb::new(config.models.clone(), &config.providers);
    let session_manager = Arc::new(SqliteSessionManager::new(pool));

    Arc::new(routes::AppState {
        config,
        registry,
        model_db,
        session_manager,
        credential_helper: "none".to_string(),
        session_db_path: std::path::PathBuf::from("/tmp/test_switchboard.db"),
        started_at: std::time::Instant::now(),
        mutations,
    })
}

fn chat_request() -> axum::http::Request<axum::body::Body> {
    let body = serde_json::to_vec(&json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": false,
    }))
    .unwrap();
    axum::http::Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap()
}

async fn upstream_body(mock_server: &wiremock::MockServer) -> serde_json::Value {
    let received = mock_server.received_requests().await.unwrap();
    serde_json::from_slice(&received[0].body).unwrap()
}

async fn upstream_header(mock_server: &wiremock::MockServer, name: &str) -> Option<String> {
    let received = mock_server.received_requests().await.unwrap();
    received[0]
        .headers
        .get(name)
        .map(|value| value.to_str().unwrap().to_string())
}

#[tokio::test]
async fn a_provider_mutation_sets_a_header_upstream() {
    let mock_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/chat/completions"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(
            r#"{"choices":[{"message":{"role":"assistant","content":"Hi"},"index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":10}}"#,
            "application/json",
        ))
        .mount(&mock_server)
        .await;

    let state = state(&mock_server.uri(), vec![fixture("header.rhai")]).await;
    let mut app = routes::build_router(state);
    let response = tower::Service::call(&mut app, chat_request())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(
        upstream_header(&mock_server, "x-fixture-provider")
            .await
            .as_deref(),
        Some("mock_openai")
    );
}

#[tokio::test]
async fn a_provider_mutation_adds_tools_upstream() {
    let mock_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/chat/completions"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(
            r#"{"choices":[{"message":{"role":"assistant","content":"Hi"},"index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":10}}"#,
            "application/json",
        ))
        .mount(&mock_server)
        .await;

    let state = state(&mock_server.uri(), vec![fixture("tools.rhai")]).await;
    let mut app = routes::build_router(state);
    let response = tower::Service::call(&mut app, chat_request())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let upstream = upstream_body(&mock_server).await;
    assert_eq!(
        upstream["tools"][0]["function"]["name"], "bash",
        "the mutation script must supply the tools"
    );
    assert_eq!(upstream["tool_choice"], "none");
}

#[tokio::test]
async fn two_mutations_run_in_declared_order_upstream() {
    let mock_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/chat/completions"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(
            r#"{"choices":[{"message":{"role":"assistant","content":"Hi"},"index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":10}}"#,
            "application/json",
        ))
        .mount(&mock_server)
        .await;

    let state = state(
        &mock_server.uri(),
        vec![fixture("order-first.rhai"), fixture("order-second.rhai")],
    )
    .await;
    let mut app = routes::build_router(state);
    let response = tower::Service::call(&mut app, chat_request())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(
        upstream_header(&mock_server, "x-order").await.as_deref(),
        Some("first-second")
    );
}

#[tokio::test]
async fn an_invalid_mutation_header_fails_the_request() {
    let mock_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/chat/completions"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw("{}", "application/json"))
        .mount(&mock_server)
        .await;

    let state = state(&mock_server.uri(), vec![fixture("invalid-header.rhai")]).await;
    let mut app = routes::build_router(state);
    let response = tower::Service::call(&mut app, chat_request())
        .await
        .unwrap();

    assert_fail_closed(response, "an invalid mutation must fail closed").await;
    assert!(
        mock_server.received_requests().await.unwrap().is_empty(),
        "an invalid mutation must fail closed"
    );
}

#[tokio::test]
async fn a_broken_mutation_fails_the_request() {
    let mock_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/chat/completions"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw("{}", "application/json"))
        .mount(&mock_server)
        .await;

    let state = state(&mock_server.uri(), vec![fixture("broken.rhai")]).await;
    let mut app = routes::build_router(state);
    let response = tower::Service::call(&mut app, chat_request())
        .await
        .unwrap();

    assert_fail_closed(response, "a script error must fail closed").await;
    assert!(
        mock_server.received_requests().await.unwrap().is_empty(),
        "a script error must fail closed"
    );
}

/// The attempt fails with 502 upstream-unreachable semantics and the outer
/// handler reports the exhausted provider list as 503; either way nothing was
/// forwarded and the attempt detail carries the mutation failure status.
async fn assert_fail_closed(response: axum::response::Response, message: &str) {
    assert_ne!(response.status(), 200, "{message}");
    let status = response.status().as_u16();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("502") || status == 502,
        "{message}: expected the mutation failure to surface, got {status}: {text}"
    );
}

#[tokio::test]
async fn a_provider_without_mutations_passes_through_unchanged() {
    let mock_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/chat/completions"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(
            r#"{"choices":[{"message":{"role":"assistant","content":"Hi"},"index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":10}}"#,
            "application/json",
        ))
        .mount(&mock_server)
        .await;

    let state = state(&mock_server.uri(), Vec::new()).await;
    let mut app = routes::build_router(state);
    let response = tower::Service::call(&mut app, chat_request())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let upstream = upstream_body(&mock_server).await;
    assert!(
        upstream.get("tools").is_none(),
        "no tools should be injected"
    );
    assert!(upstream.get("tool_choice").is_none());
}

#[test]
fn the_request_object_exposes_context_without_method_or_url() {
    let mut provider = config(vec![fixture("keys.rhai")]);
    provider.providers.get_mut("mock_openai").unwrap().base_url =
        "https://api.example.com/v1".to_string();
    let mut sets = compiled(&provider);
    let set = sets.remove("mock_openai").unwrap();

    let mut headers = axum::http::HeaderMap::new();
    let mut body = br#"{"model":"gpt-4o"}"#.to_vec();
    let context = MutationContext {
        provider_identity: "mock_openai".to_string(),
        surface: ApiSurface::OpenaiChatCompletions,
        model: "gpt-4o".to_string(),
        session_id: None,
    };

    set.apply(&context, &mut headers, &mut body).unwrap();
    let observed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let keys = &observed["keys"];

    assert_eq!(keys["provider"], "mock_openai");
    assert_eq!(keys["surface"], "openai-chat-completions");
    assert_eq!(keys["model"], "gpt-4o");
    assert_eq!(keys["session_id"], serde_json::Value::Null);
    assert_eq!(keys["body"], json!({"model": "gpt-4o"}));
    assert!(keys.get("method").is_none(), "method must be hidden");
    assert!(keys.get("url").is_none(), "url must be hidden");
}
