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
