pub mod env;
pub mod helper;

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
}
