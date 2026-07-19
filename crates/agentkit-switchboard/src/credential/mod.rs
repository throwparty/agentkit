pub mod helper;

use crate::auth::AuthType;
use crate::config::ProviderConfig;
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

    helper::get(helper_name, identity)
}
