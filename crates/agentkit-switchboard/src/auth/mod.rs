pub mod openai_codex;

use crate::config::SwitchboardConfig;
use crate::credential;
use crate::credential::helper;
use crate::credential::{CredentialSource, ResolvedCredential};
use serde::{Deserialize, Serialize};

const CODEX_DEFAULT_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CLIENT_ID_OVERRIDE_ENV: &str = "CODEX_APP_SERVER_LOGIN_CLIENT_ID";

fn default_client_id() -> String {
    std::env::var(CLIENT_ID_OVERRIDE_ENV)
        .ok()
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(|| CODEX_DEFAULT_CLIENT_ID.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub r#type: AuthType,
    pub oauth: Option<OAuthEndpointConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthType {
    #[serde(rename = "bearer_token")]
    BearerToken,
    #[serde(rename = "none")]
    None,
}

impl std::fmt::Display for AuthType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BearerToken => write!(f, "bearer_token"),
            Self::None => write!(f, "none"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthEndpointConfig {
    pub authorize_url: String,
    pub token_url: String,
    pub scopes: Option<String>,
    #[serde(default = "default_client_id")]
    pub client_id: String,
}

pub enum AuthCommand {
    Login { identity: String },
    Add { identity: String, value: String },
    Status { identity: Option<String> },
    Token { identity: String },
    Logout { identity: String },
}

pub async fn handle_auth(cmd: AuthCommand, config: &SwitchboardConfig) -> Result<String, String> {
    match cmd {
        AuthCommand::Login { identity } => openai_codex::login(&identity, config).await,
        AuthCommand::Add { identity, value } => add_credential(&identity, &value, config),
        AuthCommand::Status { identity } => Ok(status_output(config, identity.as_deref())),
        AuthCommand::Token { identity } => {
            let helper_name = config.credential_helper.as_deref().unwrap_or("keychain");
            let provider = config.providers.get(&identity).ok_or_else(|| {
                format!("provider '{identity}' not found in config")
            })?;
            credential::resolve_provider(helper_name, &identity, provider)
                .map(|credential| credential.value)
                .ok_or_else(|| format!("credential for '{identity}' not found"))
        }
        AuthCommand::Logout { identity } => openai_codex::logout(&identity, config).await,
    }
}

pub async fn maybe_refresh_credential(
    identity: &str,
    credential: ResolvedCredential,
    config: &SwitchboardConfig,
) -> Result<ResolvedCredential, String> {
    let provider = match config.providers.get(identity) {
        Some(p) => p,
        None => return Ok(credential),
    };
    let Some(oauth_cfg) = provider.auth.oauth.as_ref() else {
        return Ok(credential);
    };
    let helper_name = config.credential_helper.as_deref().unwrap_or("keychain");
    openai_codex::refresh_if_needed(identity, credential, oauth_cfg, helper_name).await
}

fn add_credential(
    identity: &str,
    value: &str,
    config: &SwitchboardConfig,
) -> Result<String, String> {
    let helper_name = config.credential_helper.as_deref().unwrap_or("keychain");
    let cred = ResolvedCredential {
        value: value.to_string(),
        source: CredentialSource::Helper {
            helper_name: helper_name.to_string(),
        },
        oauth: None,
    };
    if helper::put(helper_name, identity, &cred) {
        Ok(format!(
            "✓ Credential stored for '{identity}' (helper: agentkit-credential-{helper_name})."
        ))
    } else {
        Err(format!(
            "credential helper 'agentkit-credential-{helper_name}' not found.\n  Searched PATH:\n  {}\n  (target: agentkit-credential-{helper_name})",
            crate::credential::helper::format_path_for_display(),
        ))
    }
}

fn status_output(config: &SwitchboardConfig, filter: Option<&str>) -> String {
    let mut out = String::new();
    let helper_name = config.credential_helper.as_deref().unwrap_or("keychain");
    out.push_str(&format!("Credential helper: agentkit-credential-{helper_name}\n"));
    for (id, provider) in &config.providers {
        if let Some(f) = filter {
            if id != f {
                continue;
            }
        }
        let auth_type = provider.auth.r#type.to_string();
        let oauth = if provider.auth.oauth.is_some() {
            "configured"
        } else {
            "not configured"
        };
        let credential = credential::resolve_provider(helper_name, id, provider);
        let source = credential
            .as_ref()
            .map(|cred| credential_source_label(&cred.source))
            .unwrap_or_else(|| "unconfigured".to_string());
        let expires = credential
            .and_then(|cred| cred.oauth.and_then(|oauth| oauth.expires_at))
            .map(|expires| expires.to_rfc3339())
            .unwrap_or_else(|| "unknown".to_string());
        out.push_str(&format!(
            "{id}: type={auth_type}, oauth={oauth}, source={source}, expires_at={expires}\n"
        ));
    }
    out
}
fn credential_source_label(source: &CredentialSource) -> String {
    match source {
        CredentialSource::Helper { helper_name } => format!("agentkit-credential-{helper_name}"),
        CredentialSource::None => "none".to_string(),
    }
}
