pub mod openai_codex;

use crate::config::SwitchboardConfig;

pub enum AuthCommand {
    Login { identity: String },
    Status { identity: Option<String> },
    Token { identity: String },
    Logout { identity: String },
}

pub async fn handle_auth(
    cmd: AuthCommand,
    config: &SwitchboardConfig,
) -> Result<String, String> {
    match cmd {
        AuthCommand::Login { identity } => openai_codex::login(&identity, config).await,
        AuthCommand::Status { identity } => Ok(status_output(config, identity.as_deref())),
        AuthCommand::Token { identity } => token_output(&identity),
        AuthCommand::Logout { identity } => openai_codex::logout(&identity, config).await,
    }
}

fn status_output(config: &SwitchboardConfig, filter: Option<&str>) -> String {
    let mut out = String::new();
    for (id, provider) in &config.providers {
        if let Some(f) = filter {
            if id != f {
                continue;
            }
        }
        let auth_type = provider.auth.r#type.to_string();
        let env = crate::config::credential_env_var(&provider.identity, &provider.auth.r#type)
            .unwrap_or_else(|| "none".into());
        let oauth = if provider.auth.oauth.is_some() { "configured" } else { "not configured" };
        out.push_str(&format!("{id}: type={auth_type}, env={env}, oauth={oauth}\n"));
    }
    out
}

fn token_output(identity: &str) -> Result<String, String> {
    let var_name = format!("AGENTKIT_SWITCHBOARD_{}", identity.to_uppercase());
    match std::env::var(&var_name) {
        Ok(val) => Ok(format!("{var_name}={val}")),
        Err(_) => Err(format!("{var_name} not set")),
    }
}
