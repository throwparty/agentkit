use std::path::Path;

fn fixture_path(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

use agentkit_switchboard::auth::{AuthConfig, AuthType};
use agentkit_switchboard::config::{
    ApiSurface, BillingModel, PricingConfig, ProviderConfig,
};
use agentkit_switchboard::credential::helper;
use agentkit_switchboard::credential::{CredentialSource, ResolvedCredential};

#[test]
fn credential_parse_json_valid() {
    let json = r#"{"access_token": "tok_abc", "refresh_token": "ref_xyz", "expires_at": "2026-12-31T23:59:59Z"}"#;
    let result = helper::parse_credential_json(json, CredentialSource::None).unwrap();
    assert_eq!(result.value, "tok_abc");
    let oauth = result.oauth.unwrap();
    assert_eq!(oauth.refresh_token.unwrap(), "ref_xyz");
    assert!(oauth.expires_at.is_some());
}

#[test]
fn credential_parse_json_no_refresh() {
    let json = r#"{"access_token": "tok_abc"}"#;
    let result = helper::parse_credential_json(json, CredentialSource::None).unwrap();
    assert_eq!(result.value, "tok_abc");
    let oauth = result.oauth.unwrap();
    assert!(oauth.refresh_token.is_none());
    assert!(oauth.expires_at.is_none());
}

#[test]
fn credential_parse_json_missing_access_token() {
    let json = r#"{"refresh_token": "ref_xyz"}"#;
    let result = helper::parse_credential_json(json, CredentialSource::None);
    assert!(result.is_none());
}

#[test]
fn credential_parse_json_invalid() {
    let result = helper::parse_credential_json("not json", CredentialSource::None);
    assert!(result.is_none());
}

#[test]
fn credential_helper_missing_binary() {
    let result = helper::get("nonexistent-helper-xyz", "test_identity");
    assert!(result.is_none());
}

#[test]
fn credential_helper_store_missing_binary() {
    let cred = ResolvedCredential {
        value: "tok_test".into(),
        source: CredentialSource::None,
        oauth: None,
    };
    let result = helper::put("nonexistent-helper-xyz", "test_identity", &cred);
    assert!(!result);
}

#[test]
fn credential_helper_erase_missing_binary() {
    let result = helper::delete("nonexistent-helper-xyz", "test_identity");
    assert!(!result);
}

#[test]
fn credential_helper_get_with_mock_script() {
    let script = fixture_path("mock-helper.sh");
    if !script.exists() {
        return;
    }
    let helper_name = "test-mock";
    let binary_path = script.to_string_lossy().to_string();

    let result = std::process::Command::new(&binary_path)
        .arg("get")
        .arg("test_id")
        .output()
        .expect("mock helper should run");

    assert!(result.status.success());
    let stdout = String::from_utf8_lossy(&result.stdout);
    let parsed = helper::parse_credential_json(
        &stdout,
        CredentialSource::Helper {
            helper_name: helper_name.to_string(),
        },
    );
    assert!(
        parsed.is_some(),
        "mock helper output should parse: {stdout}"
    );
    assert_eq!(parsed.unwrap().value, "mock_access_token");
}

fn provider_with_auth(auth_type: AuthType) -> ProviderConfig {
    ProviderConfig {
        identity: "openai_api_key".to_string(),
        api_surface: ApiSurface::Openai,
        base_url: "https://api.openai.com/v1".to_string(),
        billing: BillingModel::PayAsYouGo,
        auth: AuthConfig {
            r#type: auth_type,
            oauth: None,
        },
        pricing: PricingConfig {
            input_per_mtok: 1.0,
            output_per_mtok: 2.0,
            cache_read_per_mtok: None,
            cache_write_per_mtok: None,
            reasoning_per_mtok: None,
            models: std::collections::HashMap::new(),
        },
        models: Some(vec!["gpt-4o".to_string()]),
    }
}

#[test]
fn credential_resolution_none_auth() {
    let provider = provider_with_auth(AuthType::None);
    let result =
        agentkit_switchboard::credential::resolve_provider("missing-helper", "local", &provider)
            .unwrap();
    assert!(result.value.is_empty());
    assert!(matches!(result.source, CredentialSource::None));
}
