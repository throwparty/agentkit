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
    assert_eq!(provider.api_surface.to_string(), "openai-chat-completions");
    assert_eq!(provider.billing.to_string(), "pay_as_you_go");
    assert_eq!(provider.base_url, "https://api.openai.com/v1");
    assert_eq!(provider.auth.r#type.to_string(), "bearer_token");
    assert_eq!(
        provider.auth.r#type.to_string(),
        "bearer_token"
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
    assert_eq!(oauth.client_id, "test-client-id");
}

#[test]
fn config_oauth_default_client_id() {
    let cfg = agentkit_switchboard::config::loader::load_config(
        &fixture_path("oauth-default-client.toml"),
    )
    .expect("oauth config should parse");
    let provider = cfg.providers.get("default_client_provider").unwrap();
    let oauth = provider.auth.oauth.as_ref().expect("should have oauth config");
    assert_eq!(
        oauth.client_id,
        "app_EMoamEEZ73f0CkXaXp7hrann",
        "should use upstream Codex CLI default client ID"
    );
}

#[test]
fn config_credential_helper_default() {
    let cfg =
        agentkit_switchboard::config::loader::load_config(&fixture_path("minimal.toml"))
            .expect("valid config");
    assert_eq!(cfg.credential_helper.as_deref(), Some("keychain"));
}

#[test]
fn config_e2e_zen_entries() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("e2e.toml");
    let cfg = agentkit_switchboard::config::loader::load_config(&path)
        .expect("e2e.toml should parse");

    let zen_chat = cfg.providers.get("zen_chat").expect("zen_chat entry");
    assert_eq!(zen_chat.api_surface.to_string(), "openai-chat-completions");
    assert_eq!(zen_chat.base_url, "https://opencode.ai/zen/v1");
    assert_eq!(zen_chat.billing.to_string(), "pay_as_you_go");
    assert_eq!(zen_chat.auth.r#type.to_string(), "bearer_token");

    let zen_responses = cfg.providers.get("zen_responses").expect("zen_responses entry");
    assert_eq!(zen_responses.api_surface.to_string(), "openai-responses");
    assert_eq!(zen_responses.base_url, "https://opencode.ai/zen/v1");
    assert_eq!(zen_responses.billing.to_string(), "pay_as_you_go");

    let zen_messages = cfg.providers.get("zen_messages").expect("zen_messages entry");
    assert_eq!(zen_messages.api_surface.to_string(), "anthropic-messages");
    assert_eq!(zen_messages.base_url, "https://opencode.ai/zen/v1");
    assert_eq!(zen_messages.billing.to_string(), "pay_as_you_go");

    let snapshot = agentkit_models::bundled_snapshot_parsed();
    let opencode = snapshot
        .providers
        .get("opencode")
        .expect("models.dev opencode provider should be bundled");
    for entry in [&zen_chat, &zen_responses, &zen_messages] {
        let models = entry.models.as_ref().expect("zen entry should list models");
        assert!(!models.is_empty());
        for model in models {
            assert!(
                opencode.models.contains_key(model),
                "{} model '{model}' should be served by the opencode provider and not deprecated",
                entry.identity
            );
        }
    }
}
