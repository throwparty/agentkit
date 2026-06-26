pub mod env;
pub mod helper;

use crate::config::{AuthType, ProviderConfig};
use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct ResolvedCredential {
    pub value: String,
    pub source: CredentialSource,
    pub oauth: Option<OAuthState>,
}

#[derive(Debug, Clone)]
pub enum CredentialSource {
    Helper { helper_name: String },
    EnvVar { var_name: String },
    None,
}

#[derive(Debug, Clone)]
pub struct OAuthState {
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub account_id: Option<String>,
}

pub fn resolve_provider(
    helper_name: &str,
    identity: &str,
    provider: &ProviderConfig,
) -> Option<ResolvedCredential> {
    if matches!(provider.auth.r#type, AuthType::None) {
        return Some(ResolvedCredential {
            value: String::new(),
            source: CredentialSource::None,
            oauth: None,
        });
    }

    helper::get(helper_name, identity).or_else(|| resolve_env(identity, &provider.auth.r#type))
}

pub fn resolve_env(identity: &str, auth_type: &AuthType) -> Option<ResolvedCredential> {
    for var_name in env_var_candidates(identity, auth_type) {
        if let Some(value) = env::read(&var_name) {
            return Some(ResolvedCredential {
                value,
                source: CredentialSource::EnvVar { var_name },
                oauth: None,
            });
        }
    }
    None
}

pub fn default_env_var_name(identity: &str, auth_type: &AuthType) -> String {
    let normalized = normalize_identity(identity);
    let stem = match auth_type {
        AuthType::OpenAICodexOAuth | AuthType::OAuthToken => normalized
            .strip_suffix("_SUB")
            .unwrap_or(&normalized)
            .to_string(),
        _ => normalized,
    };

    let suffix = match auth_type {
        AuthType::BearerToken | AuthType::AnthropicApiKey => "API_KEY",
        AuthType::OpenAICodexOAuth | AuthType::OAuthToken => "TOKEN",
        AuthType::None => "TOKEN",
    };

    if stem.ends_with(suffix) {
        format!("AGENTKIT_SWITCHBOARD_{stem}")
    } else {
        format!("AGENTKIT_SWITCHBOARD_{stem}_{suffix}")
    }
}

pub fn env_var_candidates(identity: &str, auth_type: &AuthType) -> Vec<String> {
    let normalized = normalize_identity(identity);
    let mut candidates = vec![default_env_var_name(identity, auth_type)];

    push_unique(
        &mut candidates,
        format!("AGENTKIT_SWITCHBOARD_{normalized}_TOKEN"),
    );
    push_unique(
        &mut candidates,
        format!("AGENTKIT_SWITCHBOARD_{normalized}"),
    );

    candidates
}

fn normalize_identity(identity: &str) -> String {
    let mut normalized = String::new();
    let mut previous_underscore = false;

    for ch in identity.chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_uppercase());
            previous_underscore = false;
        } else if !previous_underscore {
            normalized.push('_');
            previous_underscore = true;
        }
    }

    normalized.trim_matches('_').to_string()
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}
