pub mod openai_codex;

use crate::config::SwitchboardConfig;
use crate::credential;
use crate::credential::helper;
use crate::credential::{CredentialSource, ResolvedCredential};

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
        AuthCommand::Token { identity } => token_output(&identity, config),
        AuthCommand::Logout { identity } => openai_codex::logout(&identity, config).await,
    }
}

fn helper_location(config: &SwitchboardConfig) -> String {
    let name = config.credential_helper.as_deref().unwrap_or("keychain");
    match name {
        "file" => "~/.local/state/agentkit/switchboard/credentials.json".to_string(),
        "keychain" => "system keychain (service: agentkit-credential-keychain)".to_string(),
        other => format!("agentkit-credential-{other} in PATH"),
    }
}

fn warn_file_helper(name: &str) {
    if name == "file" {
        eprintln!("warning: credential helper 'file' stores credentials in plaintext on disk.");
        eprintln!("         consider using 'keychain' for better security.");
    }
}

fn add_credential(
    identity: &str,
    value: &str,
    config: &SwitchboardConfig,
) -> Result<String, String> {
    let helper_name = config.credential_helper.as_deref().unwrap_or("keychain");
    warn_file_helper(helper_name);
    let cred = ResolvedCredential {
        value: value.to_string(),
        source: CredentialSource::Helper {
            helper_name: helper_name.to_string(),
        },
        oauth: None,
    };
    if helper::store(helper_name, identity, &cred) {
        Ok(format!(
            "✓ Credential stored for '{identity}'.\n  Location: {}",
            helper_location(config)
        ))
    } else {
        Err(format!(
            "credential helper 'agentkit-credential-{helper_name}' not found.\n  Searched PATH:\n  {}\n  (target: {})",
            crate::credential::helper::format_path_for_display(),
            helper_location(config),
        ))
    }
}

fn status_output(config: &SwitchboardConfig, filter: Option<&str>) -> String {
    let mut out = String::new();
    let helper_name = config.credential_helper.as_deref().unwrap_or("keychain");
    out.push_str(&format!("Credential helper: {}\n", helper_location(config)));
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

fn token_output(identity: &str, config: &SwitchboardConfig) -> Result<String, String> {
    let helper_name = config.credential_helper.as_deref().unwrap_or("keychain");
    let provider = config
        .providers
        .get(identity)
        .ok_or_else(|| format!("provider '{identity}' not found in config"))?;
    let var_name = credential::default_env_var_name(identity, &provider.auth.r#type);
    credential::resolve_provider(helper_name, identity, provider)
        .map(|credential| format!("{var_name}={}", credential.value))
        .ok_or_else(|| format!("credential for '{identity}' not found"))
}

fn credential_source_label(source: &CredentialSource) -> String {
    match source {
        CredentialSource::Helper { helper_name } => format!("agentkit-credential-{helper_name}"),
        CredentialSource::EnvVar { var_name } => var_name.clone(),
        CredentialSource::None => "none".to_string(),
    }
}
