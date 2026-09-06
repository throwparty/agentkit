use std::collections::HashMap;
use std::sync::Arc;

use agentkit_switchboard::auth::{AuthConfig, AuthType};
use agentkit_switchboard::config::{
    ApiSurface, BillingModel, PricingConfig, ProviderConfig, SwitchboardConfig,
};
use agentkit_switchboard::models::db::ModelDb;
use agentkit_switchboard::provider::registry::ProviderRegistry;
use agentkit_switchboard::session::sqlite::SqliteSessionManager;
use agentkit_switchboard::server::routes;
use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::data::{
    AggregatedMetrics, Metric, MetricData, ResourceMetrics,
};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
use sqlx::SqlitePool;

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
    sqlx::migrate!("src/db/migrations").run(&pool).await.unwrap();
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

fn find_metric<'a>(rm: &'a ResourceMetrics, name: &str) -> Option<&'a Metric> {
    rm.scope_metrics()
        .flat_map(|sm| sm.metrics())
        .find(|m| m.name() == name)
}

fn collect_data_point_attrs<T>(data: &MetricData<T>, out: &mut Vec<Vec<KeyValue>>) {
    match data {
        MetricData::Sum(sum) => out.extend(
            sum.data_points()
                .map(|p| p.attributes().cloned().collect()),
        ),
        MetricData::Histogram(hist) => out.extend(
            hist.data_points()
                .map(|p| p.attributes().cloned().collect()),
        ),
        _ => {}
    }
}

fn find_metric_attrs(finished: &[ResourceMetrics], name: &str) -> Vec<Vec<KeyValue>> {
    let mut out = Vec::new();
    for rm in finished {
        if let Some(metric) = find_metric(rm, name) {
            match metric.data() {
                AggregatedMetrics::U64(data) => collect_data_point_attrs(data, &mut out),
                AggregatedMetrics::F64(data) => collect_data_point_attrs(data, &mut out),
                AggregatedMetrics::I64(data) => collect_data_point_attrs(data, &mut out),
            }
        }
    }
    out
}

fn has_attr(attrs: &[KeyValue], key: &str, value: &str) -> bool {
    attrs
        .iter()
        .any(|kv| kv.key.as_str() == key && kv.value.to_string() == value)
}

#[tokio::test]
async fn metrics_recorded_with_attributes() {
    let exporter = InMemoryMetricExporter::default();
    let meter_provider = SdkMeterProvider::builder()
        .with_periodic_exporter(exporter.clone())
        .build();
    opentelemetry::global::set_meter_provider(meter_provider.clone());

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

    let health = axum::http::Request::builder()
        .method("GET")
        .uri("/health")
        .body(axum::body::Body::empty())
        .unwrap();
    let health_resp = tower::Service::call(&mut app, health).await.unwrap();
    assert_eq!(health_resp.status(), 200);

    let body = serde_json::to_vec(&serde_json::json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "hi"}],
    }))
    .unwrap();
    let completions = axum::http::Request::builder()
        .method("POST")
        .uri("/openai/v1/chat/completions")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();
    let comp_resp = tower::Service::call(&mut app, completions).await.unwrap();
    assert_eq!(comp_resp.status(), 200);

    meter_provider.force_flush().unwrap();
    let finished = exporter.get_finished_metrics().unwrap();

    let requests_attrs = find_metric_attrs(&finished, "switchboard.http.requests");
    assert!(
        !requests_attrs.is_empty(),
        "switchboard.http.requests should be recorded"
    );
    assert!(
        requests_attrs.iter().any(|attrs| {
            has_attr(attrs, "method", "GET")
                && has_attr(attrs, "path", "/health")
                && has_attr(attrs, "status_code", "200")
        }),
        "http.requests should include the /health GET with status_code 200"
    );
    assert!(
        requests_attrs.iter().any(|attrs| {
            has_attr(attrs, "method", "POST")
                && has_attr(attrs, "path", "/openai/v1/chat/completions")
        }),
        "http.requests should include the chat/completions POST"
    );

    let latency_attrs = find_metric_attrs(&finished, "switchboard.provider.latency");
    assert!(
        !latency_attrs.is_empty(),
        "switchboard.provider.latency should be recorded"
    );
    assert!(
        latency_attrs.iter().any(|attrs| {
            has_attr(attrs, "provider_identity", "mock_openai")
                && has_attr(attrs, "model_name", "gpt-4o")
        }),
        "provider.latency should carry provider_identity and model_name"
    );
}