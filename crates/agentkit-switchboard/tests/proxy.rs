use std::collections::HashMap;
use std::sync::Arc;

use agentkit_switchboard::auth::{AuthConfig, AuthType};
use agentkit_switchboard::config::{
    ApiSurface, BillingModel, PricingConfig, ProviderConfig, SwitchboardConfig,
};
use agentkit_switchboard::credential::{CredentialSource, ResolvedCredential};
use agentkit_switchboard::models::db::ModelDb;
use agentkit_switchboard::provider::registry::ProviderRegistry;
use agentkit_switchboard::proxy::forwarder::{forward_request, ForwardRequest};
use agentkit_switchboard::providers::openai::{conversation::OpenAiConversation, OpenAiProvider};
use agentkit_switchboard::session::sqlite::SqliteSessionManager;
use agentkit_switchboard::server::routes;
use sqlx::SqlitePool;
use axum::http::{HeaderMap, Method};
use serde_json::json;

async fn test_state(mock_base_url: &str) -> Arc<routes::AppState> {
    let mut providers = HashMap::new();
    providers.insert(
        "mock_openai".to_string(),
        ProviderConfig {
            identity: "mock_openai".to_string(),
            api_surface: ApiSurface::Openai,
            base_url: mock_base_url.to_string(),
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
        },
    );

    let config = SwitchboardConfig {
        models: HashMap::new(),
        providers,
        credential_helper: None,
        session_db_path: None,
    };

    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!("src/db/migrations").run(&pool).await.unwrap();
    let registry = ProviderRegistry::new(&config.providers, "none");
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
    })
}

#[tokio::test]
async fn upstream_returns_correct_content_type() {
    let mock_server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/chat/completions"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_raw("data: [DONE]\n\n", "text/event-stream"),
        )
        .mount(&mock_server)
        .await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/chat/completions", mock_server.uri()))
        .header("Content-Type", "application/json")
        .body(r#"{"stream":true,"model":"gpt-4o"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "text/event-stream"
    );
}

#[tokio::test]
async fn forwarder_preserves_upstream_content_type() {
    let mock_server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/chat/completions"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_raw("data: [DONE]\n\n", "text/event-stream"),
        )
        .mount(&mock_server)
        .await;

    let credential = ResolvedCredential {
        value: String::new(),
        source: CredentialSource::None,
        oauth: None,
    };

    let body = serde_json::to_vec(&json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": true,
    }))
    .unwrap();

    let outcome = forward_request(
        ForwardRequest {
            method: Method::POST,
            headers: HeaderMap::new(),
            body: axum::body::Bytes::from(body),
            credential: &credential,
            billing: &BillingModel::PayAsYouGo,
            base_url: &mock_server.uri(),
            provider_identity: "mock_openai",
            session_id: None,
        },
        &OpenAiProvider,
        &OpenAiConversation,
    )
    .await;

    assert_eq!(outcome.status, 200);
    assert!(outcome.body_text.is_none());
    assert_eq!(
        outcome
            .response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream")
    );
}

#[tokio::test]
async fn proxy_streams_response() {
    let mock_server = wiremock::MockServer::start().await;

    // SSE chunks that mirror a typical Chat Completions streaming response
    let sse_body = concat!(
        "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"},\"index\":0}]}\n",
        "\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"index\":0}]}\n",
        "\n",
        "data: {\"choices\":[{\"delta\":{},\"index\":0,\"finish_reason\":\"stop\"}]}\n",
        "\n",
        "data: [DONE]\n",
        "\n",
    );

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/chat/completions"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_raw(sse_body, "text/event-stream"),
        )
        .mount(&mock_server)
        .await;

    let state = test_state(&mock_server.uri()).await;
    let mut app = routes::build_router(state);

    let body = serde_json::to_vec(&json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": true,
    }))
    .unwrap();

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = tower::Service::call(&mut app, request).await.unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream")
    );
    assert!(response
        .headers()
        .get("x-switchboard-provider")
        .is_some());
    assert!(response.headers().get("x-switchboard-billing").is_some());

    let body_bytes = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let body_text = String::from_utf8(body_bytes.to_vec()).unwrap();
    assert!(body_text.contains("Hello"), "body: {body_text:?}");
    assert!(body_text.contains("[DONE]"), "body: {body_text:?}");
}

#[tokio::test]
async fn proxy_non_streaming_response() {
    let mock_server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/chat/completions"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_raw(
                    r#"{"choices":[{"message":{"role":"assistant","content":"Hi"},"index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":10}}"#,
                    "application/json",
                ),
        )
        .mount(&mock_server)
        .await;

    let state = test_state(&mock_server.uri()).await;
    let mut app = routes::build_router(state);

    let body = serde_json::to_vec(&json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": false,
    }))
    .unwrap();

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = tower::Service::call(&mut app, request).await.unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
    assert!(response
        .headers()
        .get("x-switchboard-provider")
        .is_some());

    let body_bytes = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let body_text = String::from_utf8(body_bytes.to_vec()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&body_text).unwrap();
    assert_eq!(parsed["choices"][0]["message"]["content"], "Hi");
}

#[tokio::test]
async fn proxy_unknown_model_503() {
    let mock_server = wiremock::MockServer::start().await;
    let state = test_state(&mock_server.uri()).await;
    let mut app = routes::build_router(state);

    let body = serde_json::to_vec(&json!({
        "model": "nonexistent-model",
        "messages": [{"role": "user", "content": "hi"}],
    }))
    .unwrap();

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = tower::Service::call(&mut app, request).await.unwrap();
    assert_eq!(response.status(), 503);
}
