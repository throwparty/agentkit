use agentkit_switchboard::auth::{AuthConfig, AuthType};
use agentkit_switchboard::config::{
    ApiSurface, BillingModel, ModelConfig, PricingConfig, ProviderConfig,
};
use agentkit_switchboard::models::db::ModelDb;
use std::collections::HashMap;

fn make_provider(id: &str, models: Vec<&str>) -> ProviderConfig {
    make_provider_with_overrides(id, models, HashMap::new())
}

fn make_provider_with_overrides(
    id: &str,
    models: Vec<&str>,
    model_overrides: HashMap<String, ModelConfig>,
) -> ProviderConfig {
    ProviderConfig {
        mutations: Vec::new(),
        identity: id.to_string(),
        api_surface: ApiSurface::OpenaiChatCompletions,
        base_url: "https://api.openai.com/v1".into(),
        billing: BillingModel::PayAsYouGo,
        auth: AuthConfig {
            r#type: AuthType::None,
            oauth: None,
        },
        pricing: PricingConfig {
            input_per_mtok: 2.50,
            output_per_mtok: 10.00,
            cache_read_per_mtok: None,
            cache_write_per_mtok: None,
            reasoning_per_mtok: None,
            models: HashMap::new(),
        },
        models: Some(models.into_iter().map(|m| m.to_string()).collect()),
        model_overrides,
    }
}

async fn model_rows(providers: HashMap<String, ProviderConfig>) -> Vec<serde_json::Value> {
    use agentkit_switchboard::config::SwitchboardConfig;
    use agentkit_switchboard::provider::registry::ProviderRegistry;
    use agentkit_switchboard::server::routes;
    use agentkit_switchboard::session::sqlite::SqliteSessionManager;
    use http_body_util::BodyExt as _;
    use std::sync::Arc;

    let config = SwitchboardConfig {
        providers,
        credential_helper: None,
        session_db_path: None,
    };
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!("src/db/migrations")
        .run(&pool)
        .await
        .unwrap();
    let registry = ProviderRegistry::new(&config.providers, "none").unwrap();
    let state = Arc::new(routes::AppState {
        model_db: ModelDb::new(&config.providers),
        config,
        registry,
        session_manager: Arc::new(SqliteSessionManager::new(pool)),
        credential_helper: "none".to_string(),
        session_db_path: std::path::PathBuf::from("/tmp/test_switchboard.db"),
        started_at: std::time::Instant::now(),
        mutations: HashMap::new(),
    });
    let mut app = routes::build_router(state);
    let request = axum::http::Request::builder()
        .method("GET")
        .uri("/openai/v1/models")
        .body(axum::body::Body::empty())
        .unwrap();
    let response = tower::Service::call(&mut app, request).await.unwrap();
    assert_eq!(response.status(), 200);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    json["data"].as_array().unwrap().clone()
}

#[test]
fn models_lookup_found() {
    let mut providers = HashMap::new();
    providers.insert(
        "test_provider".into(),
        make_provider("test_provider", vec!["gpt-4o"]),
    );
    let db = ModelDb::new(&providers);
    let model = db
        .lookup_for("test_provider", "gpt-4o")
        .expect("gpt-4o should be found");
    assert_eq!(model.id, "test_provider/gpt-4o");
    assert_eq!(model.providers[0].identity, "test_provider");
}

#[test]
fn models_with_matching_complete_shapes_get_a_combined_alias() {
    let mut providers = HashMap::new();
    providers.insert("first".into(), make_provider("first", vec!["gpt-4o"]));
    providers.insert("second".into(), make_provider("second", vec!["gpt-4o"]));
    let db = ModelDb::new(&providers);

    let alias = db.lookup("gpt-4o").expect("combined alias is exposed");
    assert!(alias.alias);
    assert_eq!(
        alias
            .providers
            .iter()
            .map(|provider| provider.identity.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert!(db.lookup_for("first", "gpt-4o").is_some());
    assert!(db.lookup_for("second", "gpt-4o").is_some());
}

#[test]
fn models_with_unknown_shapes_do_not_combine() {
    let mut first_overrides = HashMap::new();
    first_overrides.insert(
        "custom-model".into(),
        ModelConfig {
            context_window: Some(1_000),
            max_output: Some(100),
            capabilities: None,
        },
    );
    let mut second_overrides = HashMap::new();
    second_overrides.insert(
        "custom-model".into(),
        ModelConfig {
            context_window: Some(1_000),
            max_output: Some(100),
            capabilities: None,
        },
    );
    let mut providers = HashMap::new();
    providers.insert(
        "first".into(),
        make_provider_with_overrides("first", vec!["custom-model"], first_overrides),
    );
    providers.insert(
        "second".into(),
        make_provider_with_overrides("second", vec!["custom-model"], second_overrides),
    );
    let db = ModelDb::new(&providers);

    assert!(db.lookup("custom-model").is_none());
    assert!(db.lookup_for("first", "custom-model").is_some());
    assert!(db.lookup_for("second", "custom-model").is_some());
}

#[test]
fn a_single_unknown_shape_still_gets_a_bare_alias() {
    let mut overrides = HashMap::new();
    overrides.insert(
        "custom-model".into(),
        ModelConfig {
            context_window: Some(1_000),
            max_output: Some(100),
            capabilities: None,
        },
    );
    let mut providers = HashMap::new();
    providers.insert(
        "only".into(),
        make_provider_with_overrides("only", vec!["custom-model"], overrides),
    );
    let db = ModelDb::new(&providers);

    let alias = db.lookup("custom-model").expect("single group alias");
    assert!(alias.alias);
    assert_eq!(alias.context_window, Some(1_000));
    assert_eq!(alias.capabilities, None);
}

#[test]
fn models_with_different_shapes_do_not_get_a_bare_alias() {
    let mut overrides = HashMap::new();
    overrides.insert(
        "gpt-4o".into(),
        ModelConfig {
            context_window: Some(999999),
            max_output: None,
            capabilities: None,
        },
    );
    let mut providers = HashMap::new();
    providers.insert(
        "custom_provider".into(),
        make_provider_with_overrides("custom_provider", vec!["gpt-4o"], overrides),
    );
    providers.insert(
        "stock_provider".into(),
        make_provider("stock_provider", vec!["gpt-4o"]),
    );
    let db = ModelDb::new(&providers);

    assert!(db.lookup("gpt-4o").is_none());
    assert!(db.lookup_for("custom_provider", "gpt-4o").is_some());
    assert!(db.lookup_for("stock_provider", "gpt-4o").is_some());
}

#[test]
fn models_lookup_missing() {
    let db = ModelDb::new(&HashMap::new());
    assert!(db.lookup("nonexistent-model").is_none());
}

#[test]
fn models_merge_provider_override() {
    let mut overrides = HashMap::new();
    overrides.insert(
        "gpt-4o".into(),
        ModelConfig {
            context_window: Some(999999),
            max_output: None,
            capabilities: None,
        },
    );
    let mut providers = HashMap::new();
    providers.insert(
        "custom_provider".into(),
        make_provider_with_overrides("custom_provider", vec!["gpt-4o"], overrides),
    );
    let db = ModelDb::new(&providers);
    let model = db.lookup_for("custom_provider", "gpt-4o").unwrap();
    assert_eq!(model.context_window, Some(999999));
    assert_eq!(model.max_output, Some(16384));
}

#[test]
fn models_override_does_not_leak_to_other_providers() {
    let mut overrides = HashMap::new();
    overrides.insert(
        "gpt-4o".into(),
        ModelConfig {
            context_window: Some(999999),
            max_output: None,
            capabilities: None,
        },
    );
    let mut providers = HashMap::new();
    providers.insert(
        "custom_provider".into(),
        make_provider_with_overrides("custom_provider", vec!["gpt-4o"], overrides),
    );
    providers.insert(
        "stock_provider".into(),
        make_provider("stock_provider", vec!["gpt-4o"]),
    );

    let db = ModelDb::new(&providers);
    let overridden = db.lookup_for("custom_provider", "gpt-4o").unwrap();
    let stock = db.lookup_for("stock_provider", "gpt-4o").unwrap();
    assert_eq!(overridden.context_window, Some(999999));
    assert_eq!(stock.context_window, Some(128000));
}

#[test]
fn models_provider_pricing() {
    let mut providers = HashMap::new();
    providers.insert(
        "custom_provider".into(),
        make_provider("custom_provider", vec!["gpt-4o"]),
    );
    let db = ModelDb::new(&providers);
    let model = db.lookup_for("custom_provider", "gpt-4o").unwrap();
    let pricing = model.providers[0].pricing.as_ref().unwrap();
    assert_eq!(pricing.input_per_mtok, 2.50);
    assert_eq!(pricing.output_per_mtok, 10.00);
}

#[test]
fn models_load_external_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.dev.json");
    let snapshot = serde_json::json!({
        "models": {
            "external-model": {
                "context_window": 1234,
                "max_output": 56,
                "capabilities": {
                    "tool_calling": true,
                    "reasoning": false,
                    "structured_output": true
                }
            }
        },
        "providers": {}
    });
    std::fs::write(&path, serde_json::to_vec(&snapshot).unwrap()).unwrap();

    let mut providers = HashMap::new();
    providers.insert(
        "external_provider".into(),
        make_provider("external_provider", vec!["external-model"]),
    );
    let db = ModelDb::from_snapshot_path(&path, &providers).unwrap();
    let model = db.lookup("external-model").unwrap();
    assert_eq!(model.context_window, Some(1234));
    assert_eq!(model.max_output, Some(56));
    let capabilities = model.capabilities.as_ref().unwrap();
    assert_eq!(capabilities.tool_calling, Some(true));
    assert_eq!(capabilities.reasoning, Some(false));
    assert_eq!(capabilities.structured_output, Some(true));
}

#[tokio::test]
async fn models_override_is_forwarded_by_the_models_route() {
    let mut overrides = HashMap::new();
    overrides.insert(
        "deepseek-v4.1-flash".into(),
        ModelConfig {
            context_window: Some(131_072),
            max_output: Some(8_192),
            capabilities: None,
        },
    );
    let mut alternate_overrides = HashMap::new();
    alternate_overrides.insert(
        "deepseek-v4.1-flash".into(),
        ModelConfig {
            context_window: Some(1_000_000),
            max_output: Some(8_192),
            capabilities: None,
        },
    );
    let mut providers = HashMap::new();
    providers.insert(
        "zen_chat".into(),
        make_provider_with_overrides("zen_chat", vec!["deepseek-v4.1-flash"], overrides),
    );
    providers.insert(
        "zen_chat_alt".into(),
        make_provider_with_overrides(
            "zen_chat_alt",
            vec!["deepseek-v4.1-flash"],
            alternate_overrides,
        ),
    );
    let models = model_rows(providers).await;
    assert!(
        models
            .iter()
            .all(|model| model["id"] != "deepseek-v4.1-flash"),
        "unknown shape prevents a combined alias"
    );
    let primary = models
        .iter()
        .find(|model| model["id"] == "zen_chat/deepseek-v4.1-flash")
        .expect("primary provider/model instance is listed");
    assert_eq!(primary["provider"], "zen_chat");
    assert_eq!(primary["context_window"], 131_072);
    assert_eq!(primary["max_output"], 8_192);
    let alternate = models
        .iter()
        .find(|model| model["id"] == "zen_chat_alt/deepseek-v4.1-flash")
        .expect("alternate provider/model instance is listed");
    assert_eq!(alternate["provider"], "zen_chat_alt");
    assert_eq!(alternate["context_window"], 1_000_000);
}

#[tokio::test]
async fn models_combined_alias_is_forwarded_by_the_models_route() {
    let mut providers = HashMap::new();
    providers.insert("first".into(), make_provider("first", vec!["gpt-4o"]));
    providers.insert("second".into(), make_provider("second", vec!["gpt-4o"]));

    let models = model_rows(providers).await;
    let alias = models
        .iter()
        .find(|model| model["id"] == "gpt-4o")
        .expect("combined alias is listed");
    assert_eq!(alias["model"], "gpt-4o");
    assert!(alias["provider"].is_null());
    assert!(alias["billing"].is_null());
    assert!(alias["pricing"].is_null());
    let providers = alias["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 2);
    assert_eq!(providers[0]["identity"], "first");
    assert_eq!(providers[1]["identity"], "second");
}
