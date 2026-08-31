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
use agentkit_switchboard::providers::openai::OpenAiChatCompletionsProvider;
use agentkit_switchboard::session::sqlite::SqliteSessionManager;
use agentkit_switchboard::server::routes;
use sqlx::SqlitePool;
use axum::http::{HeaderMap, Method};
use serde_json::json;

async fn test_state(mock_base_url: &str) -> Arc<routes::AppState> {
    test_state_with(
        mock_base_url,
        "mock_openai",
        ApiSurface::OpenaiChatCompletions,
        vec!["gpt-4o"],
    )
    .await
}

async fn test_state_with(
    mock_base_url: &str,
    identity: &str,
    surface: ApiSurface,
    models: Vec<&str>,
) -> Arc<routes::AppState> {
    let mut providers = HashMap::new();
    providers.insert(
        identity.to_string(),
        ProviderConfig {
            identity: identity.to_string(),
            api_surface: surface,
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
            models: Some(models.into_iter().map(|m| m.to_string()).collect()),
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
        &OpenAiChatCompletionsProvider,
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

#[test]
fn anthropic_messages_presents_key_via_x_api_key() {
    use agentkit_switchboard::credential::{CredentialSource, ResolvedCredential};
    use agentkit_switchboard::domain::http::HttpEndpoint;
    use agentkit_switchboard::providers::anthropic::AnthropicProvider;
    use axum::http::HeaderMap;

    let credential = ResolvedCredential {
        value: "sk-zen-test".to_string(),
        source: CredentialSource::Helper {
            helper_name: "keychain".to_string(),
        },
        oauth: None,
    };
    let provider = AnthropicProvider;
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        axum::http::HeaderValue::from_static("Bearer inbound-token"),
    );
    provider.inject_headers(&mut headers, &credential);

    assert_eq!(
        headers.get("x-api-key").map(|v| v.to_str().unwrap()),
        Some("sk-zen-test"),
        "Zen's /messages endpoint requires x-api-key, not Authorization: Bearer"
    );
    assert!(
        headers.get("authorization").is_none(),
        "authorization header should be stripped"
    );
    assert_eq!(
        headers
            .get("anthropic-version")
            .map(|v| v.to_str().unwrap()),
        Some("2023-06-01")
    );
}

#[tokio::test]
async fn proxy_responses_endpoint_routes_to_responses_provider() {
    let mock_server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/responses"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_raw(
                r#"{"id":"resp_1","object":"response","output":[{"type":"message","content":[{"type":"output_text","text":"Hi"}]}],"usage":{"input_tokens":5,"output_tokens":10}}"#,
                "application/json",
            ),
        )
        .mount(&mock_server)
        .await;

    let state = test_state_with(
        &mock_server.uri(),
        "mock_responses",
        ApiSurface::OpenaiResponses,
        vec!["gpt-4o"],
    )
    .await;
    let mut app = routes::build_router(state);

    let body = serde_json::to_vec(&json!({
        "model": "gpt-4o",
        "input": "hi",
    }))
    .unwrap();

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/openai/v1/responses")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = tower::Service::call(&mut app, request).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response
            .headers()
            .get("x-switchboard-provider")
            .and_then(|v| v.to_str().ok()),
        Some("mock_responses")
    );
}

#[tokio::test]
async fn proxy_responses_endpoint_never_selects_chat_completions() {
    let mock_server = wiremock::MockServer::start().await;
    let state = test_state(&mock_server.uri()).await;
    let mut app = routes::build_router(state);

    let body = serde_json::to_vec(&json!({
        "model": "gpt-4o",
        "input": "hi",
    }))
    .unwrap();

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/openai/v1/responses")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = tower::Service::call(&mut app, request).await.unwrap();
    assert_eq!(response.status(), 503);
}

#[tokio::test]
async fn proxy_messages_endpoint_routes_to_messages_provider() {
    let mock_server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/messages"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_raw(
                r#"{"id":"msg_1","type":"message","role":"assistant","model":"gpt-4o","content":[{"type":"text","text":"Hi"}],"usage":{"input_tokens":5,"output_tokens":10}}"#,
                "application/json",
            ),
        )
        .mount(&mock_server)
        .await;

    let state = test_state_with(
        &mock_server.uri(),
        "mock_messages",
        ApiSurface::AnthropicMessages,
        vec!["gpt-4o"],
    )
    .await;
    let mut app = routes::build_router(state);

    let body = serde_json::to_vec(&json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "hi"}],
    }))
    .unwrap();

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/anthropic/v1/messages")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = tower::Service::call(&mut app, request).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response
            .headers()
            .get("x-switchboard-provider")
            .and_then(|v| v.to_str().ok()),
        Some("mock_messages")
    );
}

#[tokio::test]
async fn proxy_unknown_path_404() {
    let mock_server = wiremock::MockServer::start().await;
    let state = test_state(&mock_server.uri()).await;
    let mut app = routes::build_router(state);

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/openai/v1/unknown")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let response = tower::Service::call(&mut app, request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn proxy_request_body_passes_through_unchanged() {
    let mock_server = wiremock::MockServer::start().await;

    let sent = json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": false,
    });

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/chat/completions"))
        .and(wiremock::matchers::body_json(sent.clone()))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_raw(
                r#"{"choices":[{"message":{"role":"assistant","content":"Hi"},"index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":10}}"#,
                "application/json",
            ),
        )
        .mount(&mock_server)
        .await;

    let state = test_state(&mock_server.uri()).await;
    let mut app = routes::build_router(state);

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&sent).unwrap()))
        .unwrap();

    let response = tower::Service::call(&mut app, request).await.unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn proxy_response_body_passes_through_byte_for_byte() {
    let mock_server = wiremock::MockServer::start().await;

    let raw_body = concat!(
        "{\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"Hi\"},",
        "\"index\":0,\"finish_reason\":\"stop\"}],",
        "\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":10}}",
    );

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/chat/completions"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_raw(raw_body, "application/json"),
        )
        .mount(&mock_server)
        .await;

    let state = test_state(&mock_server.uri()).await;
    let mut app = routes::build_router(state);

    let body = serde_json::to_vec(&json!({
        "model": "gpt-4o",
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
    assert_eq!(response.status(), 200);
    let response_body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    assert_eq!(
        String::from_utf8(response_body.to_vec()).unwrap(),
        raw_body,
        "response body should pass through byte-for-byte without translation"
    );
}
