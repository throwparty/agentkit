use std::collections::HashMap;
use std::sync::Arc;

use agentkit_switchboard::auth::{AuthConfig, AuthType};
use agentkit_switchboard::config::{
    ApiSurface, BillingModel, PricingConfig, ProviderConfig, SwitchboardConfig,
};
use agentkit_switchboard::models::db::ModelDb;
use agentkit_switchboard::provider::registry::ProviderRegistry;
use agentkit_switchboard::server::routes;
use agentkit_switchboard::session::sqlite::SqliteSessionManager;
use opentelemetry::trace::{SpanId, TracerProvider};
use opentelemetry_sdk::trace::InMemorySpanExporter;
use sqlx::SqlitePool;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

async fn test_state(mock_base_url: &str) -> Arc<routes::AppState> {
    let mut providers = HashMap::new();
    providers.insert(
        "mock_openai".to_string(),
        ProviderConfig {
            identity: "mock_openai".to_string(),
            api_surface: ApiSurface::OpenaiChatCompletions,
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
    sqlx::migrate!("src/db/migrations")
        .run(&pool)
        .await
        .unwrap();
    let registry = ProviderRegistry::new(&config.providers, "none")
        .expect("none-auth provider needs no credential");
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
async fn request_produces_phase_span_tree() {
    let span_exporter = InMemorySpanExporter::default();
    let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_simple_exporter(span_exporter.clone())
        .build();
    let tracer = tracer_provider.tracer("agentkit-switchboard");

    tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .init();

    let mock_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/chat/completions"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(
            r#"{"choices":[{"message":{"role":"assistant","content":"Hi"},"index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":10}}"#,
            "application/json",
        ))
        .mount(&mock_server)
        .await;

    let state = test_state(&mock_server.uri()).await;
    let mut app = routes::build_router(state);

    let body = serde_json::to_vec(&serde_json::json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "hi"}],
    }))
    .unwrap();
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("Content-Type", "application/json")
        .header("X-Session-Id", "sess-otel-test")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = tower::Service::call(&mut app, request).await.unwrap();
    assert_eq!(response.status(), 200);

    let spans = span_exporter.get_finished_spans().unwrap();
    let root = spans
        .iter()
        .find(|s| s.name.as_ref() == "proxy_handler")
        .expect("a proxy_handler root span should be exported");
    assert_eq!(
        root.parent_span_id,
        SpanId::INVALID,
        "proxy_handler should be the trace root"
    );

    let root_id = root.span_context.span_id();
    for name in [
        "get_states",
        "forward_request",
        "record_response",
        "log_routing_event",
        "lookup",
        "assign",
        "update_tokens",
    ] {
        assert!(
            spans
                .iter()
                .any(|s| s.name.as_ref() == name && s.parent_span_id == root_id),
            "missing child span '{name}' under proxy_handler"
        );
    }
}
