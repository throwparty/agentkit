use std::collections::HashMap;
use agentkit_switchboard::config::{ModelConfig, ProviderConfig, ApiSurface, BillingModel, AuthConfig, AuthType, PricingConfig};
use agentkit_switchboard::models::db::ModelDb;

fn make_provider(id: &str, models: Vec<&str>) -> ProviderConfig {
    ProviderConfig {
        identity: id.to_string(),
        api_surface: ApiSurface::Openai,
        base_url: "https://api.openai.com/v1".into(),
        billing: BillingModel::PayAsYouGo,
        auth: AuthConfig {
            r#type: AuthType::BearerToken,
            credential_env: None,
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
    }
}

#[test]
fn models_lookup_found() {
    let mut providers = HashMap::new();
    providers.insert("test_provider".into(), make_provider("test_provider", vec!["gpt-4o"]));
    let db = ModelDb::new(HashMap::new(), &providers);
    let model = db.lookup("gpt-4o").expect("gpt-4o should be found");
    assert_eq!(model.id, "gpt-4o");
    assert_eq!(model.providers.len(), 1);
    assert_eq!(model.providers[0].identity, "test_provider");
}

#[test]
fn models_lookup_missing() {
    let db = ModelDb::new(HashMap::new(), &HashMap::new());
    assert!(db.lookup("nonexistent-model").is_none());
}

#[test]
fn models_merge_override() {
    let mut overrides = HashMap::new();
    overrides.insert("gpt-4o".into(), ModelConfig {
        context_window: Some(999999),
        max_output: None,
        capabilities: None,
    });
    let db = ModelDb::new(overrides, &HashMap::new());
    let model = db.lookup("gpt-4o").expect("gpt-4o found via override");
    assert_eq!(model.context_window, Some(999999));
}

#[test]
fn models_provider_pricing() {
    let mut providers = HashMap::new();
    providers.insert("openai".into(), make_provider("openai", vec!["gpt-4o"]));
    let db = ModelDb::new(HashMap::new(), &providers);
    let model = db.lookup("gpt-4o").unwrap();
    let prov = &model.providers[0];
    let pricing = prov.pricing.as_ref().unwrap();
    assert_eq!(pricing.input_per_mtok, 2.50);
    assert_eq!(pricing.output_per_mtok, 10.00);
}
