use std::collections::HashMap;
use agentkit_switchboard::config::{BillingModel, PricingConfig};
use agentkit_switchboard::provider::{ProviderStatus, ProviderView};
use agentkit_switchboard::session::SessionAffinity;
use agentkit_switchboard::provider::router::{select_provider, RoutingError, SelectionReason};

fn make_provider(id: &str, billing: BillingModel, models: Vec<&str>, available: bool) -> (String, ProviderView) {
    let status = if available { ProviderStatus::Healthy } else { ProviderStatus::Unauthenticated };
    (
        id.to_string(),
        ProviderView {
            identity: id.to_string(),
            billing,
            models: models.into_iter().map(|m| m.to_string()).collect(),
            has_valid_credential: available,
            status,
            pricing: PricingConfig {
                input_per_mtok: 2.50,
                output_per_mtok: 10.00,
                cache_read_per_mtok: None,
                cache_write_per_mtok: None,
                reasoning_per_mtok: None,
                models: HashMap::new(),
            },
        },
    )
}

fn make_provider_with_cost(id: &str, billing: BillingModel, cost: f64, models: Vec<&str>) -> (String, ProviderView) {
    (
        id.to_string(),
        ProviderView {
            identity: id.to_string(),
            billing,
            models: models.into_iter().map(|m| m.to_string()).collect(),
            has_valid_credential: true,
            status: ProviderStatus::Healthy,
            pricing: PricingConfig {
                input_per_mtok: cost,
                output_per_mtok: cost * 4.0,
                cache_read_per_mtok: None,
                cache_write_per_mtok: None,
                reasoning_per_mtok: None,
                models: HashMap::new(),
            },
        },
    )
}

fn providers_from(v: Vec<(String, ProviderView)>) -> HashMap<String, ProviderView> {
    v.into_iter().collect()
}

#[test]
fn routing_prefers_subscription() {
    let p = providers_from(vec![
        make_provider("payg", BillingModel::PayAsYouGo, vec!["gpt-4o"], true),
        make_provider("sub", BillingModel::Subscription, vec!["gpt-4o"], true),
    ]);
    let result = select_provider("gpt-4o", None, &p).unwrap();
    assert_eq!(result.identity, "sub");
    assert_eq!(result.reason, SelectionReason::Cost);
}

#[test]
fn routing_falls_through_on_quota_exhausted() {
    let p = providers_from(vec![
        make_provider("payg", BillingModel::PayAsYouGo, vec!["gpt-4o"], true),
        make_provider("sub", BillingModel::Subscription, vec!["gpt-4o"], false),
    ]);
    let result = select_provider("gpt-4o", None, &p).unwrap();
    assert_eq!(result.identity, "payg");
}

#[test]
fn routing_ranks_by_cost() {
    let p = providers_from(vec![
        make_provider_with_cost("expensive", BillingModel::PayAsYouGo, 10.0, vec!["gpt-4o"]),
        make_provider_with_cost("cheap", BillingModel::PayAsYouGo, 1.0, vec!["gpt-4o"]),
    ]);
    let result = select_provider("gpt-4o", None, &p).unwrap();
    assert_eq!(result.identity, "cheap");
}

#[test]
fn routing_tiebreaker_identity() {
    let p = providers_from(vec![
        make_provider_with_cost("b_provider", BillingModel::PayAsYouGo, 2.50, vec!["gpt-4o"]),
        make_provider_with_cost("a_provider", BillingModel::PayAsYouGo, 2.50, vec!["gpt-4o"]),
    ]);
    let result = select_provider("gpt-4o", None, &p).unwrap();
    assert_eq!(result.identity, "a_provider");
}

#[test]
fn routing_model_not_available() {
    let p = providers_from(vec![
        make_provider("p", BillingModel::PayAsYouGo, vec!["gpt-4o"], true),
    ]);
    let result = select_provider("nonexistent", None, &p);
    assert_eq!(result, Err(RoutingError::ModelNotFound));
}

#[test]
fn routing_no_credential() {
    let p = providers_from(vec![
        make_provider("p", BillingModel::PayAsYouGo, vec!["gpt-4o"], false),
    ]);
    let result = select_provider("gpt-4o", None, &p);
    assert_eq!(result, Err(RoutingError::NoProvider));
}

#[test]
fn routing_session_affinity() {
    let p = providers_from(vec![
        make_provider("sub", BillingModel::Subscription, vec!["gpt-4o"], true),
        make_provider("payg", BillingModel::PayAsYouGo, vec!["gpt-4o"], true),
    ]);
    let session = SessionAffinity {
        session_id: "sess_1".into(),
        provider_identity: "payg".into(),
        model_name: "gpt-4o".into(),
        api_surface: "openai".into(),
    };
    let result = select_provider("gpt-4o", Some(&session), &p).unwrap();
    assert_eq!(result.identity, "payg");
    assert_eq!(result.reason, SelectionReason::Affinity);
    assert_eq!(result.switch_count, 0);
}

#[test]
fn routing_session_breaks_on_degradation() {
    let p = providers_from(vec![
        make_provider("degraded_sub", BillingModel::Subscription, vec!["gpt-4o"], false),
        make_provider("payg", BillingModel::PayAsYouGo, vec!["gpt-4o"], true),
    ]);
    let session = SessionAffinity {
        session_id: "sess_1".into(),
        provider_identity: "degraded_sub".into(),
        model_name: "gpt-4o".into(),
        api_surface: "openai".into(),
    };
    let result = select_provider("gpt-4o", Some(&session), &p).unwrap();
    assert_eq!(result.identity, "payg");
    assert_eq!(result.reason, SelectionReason::Fallback);
    assert_eq!(result.switch_count, 1);
}

#[test]
fn routing_all_degraded() {
    let p = providers_from(vec![
        make_provider("p1", BillingModel::PayAsYouGo, vec!["gpt-4o"], false),
        make_provider("p2", BillingModel::Subscription, vec!["gpt-4o"], false),
    ]);
    let result = select_provider("gpt-4o", None, &p);
    assert_eq!(result, Err(RoutingError::NoProvider));
}
