use std::path::Path;

fn fixture_path(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn config_parse_valid() {
    let cfg = agentkit_switchboard::config::loader::load_config(&fixture_path("minimal.toml"))
        .expect("valid config should parse");
    assert_eq!(cfg.providers.len(), 1);
    let provider = cfg.providers.get("test_provider").unwrap();
    assert_eq!(provider.api_surface.to_string(), "openai");
    assert_eq!(provider.billing.to_string(), "pay_as_you_go");
    assert_eq!(provider.base_url, "https://api.openai.com/v1");
    assert_eq!(provider.auth.r#type.to_string(), "bearer_token");
    assert_eq!(
        provider.auth.credential_env.as_deref(),
        Some("TEST_API_KEY")
    );
    assert_eq!(cfg.credential_helper.as_deref(), Some("keychain"));
    assert_eq!(cfg.models.len(), 1);
    assert!(cfg.models.contains_key("gpt-4o"));
}

#[test]
fn config_parse_duplicate_identity() {
    let err = agentkit_switchboard::config::loader::load_config(
        &fixture_path("duplicate-identity.toml"),
    )
    .expect_err("duplicate identity should fail");
    let msg = err.to_string();
    assert!(
        msg.contains("dup_provider"),
        "error should mention duplicate identity: {msg}"
    );
}

#[test]
fn config_parse_bad_enum() {
    let err =
        agentkit_switchboard::config::loader::load_config(&fixture_path("bad-enum.toml"))
            .expect_err("bad enum should fail");
    assert!(
        err.to_string().contains("unknown variant")
            || err.to_string().contains("billing")
            || err.to_string().contains("monthly"),
        "error should mention the unknown variant: {}",
        err
    );
}

#[test]
fn config_oauth_endpoints() {
    let cfg = agentkit_switchboard::config::loader::load_config(&fixture_path("oauth-config.toml"))
        .expect("oauth config should parse");
    let provider = cfg.providers.get("oauth_provider").unwrap();
    let oauth = provider.auth.oauth.as_ref().expect("should have oauth config");
    assert_eq!(oauth.authorize_url, "https://auth.openai.com/oauth/authorize");
    assert_eq!(oauth.token_url, "https://auth.openai.com/oauth/token");
    assert_eq!(
        oauth.scopes.as_deref(),
        Some("openid profile email offline_access")
    );
}

#[test]
fn config_credential_helper_default() {
    let cfg =
        agentkit_switchboard::config::loader::load_config(&fixture_path("minimal.toml"))
            .expect("valid config");
    assert_eq!(cfg.credential_helper.as_deref(), Some("keychain"));
}
