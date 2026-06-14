use std::process::Command;
use chrono::{DateTime, Utc};
use crate::credential::{CredentialSource, OAuthState, ResolvedCredential};

fn helper_binary_path(helper_name: &str) -> String {
    format!("agentkit-credential-{helper_name}")
}

pub fn parse_credential_json(json: &str, source: CredentialSource) -> Option<ResolvedCredential> {
    let parsed: serde_json::Value = serde_json::from_str(json).ok()?;
    let access_token = parsed.get("access_token")?.as_str()?.to_string();
    let refresh_token = parsed.get("refresh_token").and_then(|v| v.as_str()).map(|s| s.to_string());
    let expires_at = parsed
        .get("expires_at")
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    Some(ResolvedCredential {
        value: access_token,
        source,
        oauth: Some(OAuthState { refresh_token, expires_at }),
    })
}

pub fn get(helper_name: &str, identity: &str) -> Option<ResolvedCredential> {
    let binary = helper_binary_path(helper_name);
    let output = Command::new(&binary)
        .arg("get")
        .arg(identity)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let source = CredentialSource::Helper {
        helper_name: helper_name.to_string(),
    };
    parse_credential_json(&stdout, source)
}

pub fn store(helper_name: &str, identity: &str, credential: &ResolvedCredential) -> bool {
    let binary = helper_binary_path(helper_name);
    let json = serde_json::json!({
        "access_token": credential.value,
        "refresh_token": credential.oauth.as_ref().and_then(|o| o.refresh_token.as_deref()),
        "expires_at": credential.oauth.as_ref()
            .and_then(|o| o.expires_at)
            .map(|dt| dt.to_rfc3339()),
    });

    let body = serde_json::to_string(&json).unwrap_or_default();
    let output = Command::new(&binary)
        .arg("store")
        .arg(identity)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(body.as_bytes())?;
            }
            child.wait_with_output()
        });

    matches!(output, Ok(o) if o.status.success())
}

pub fn erase(helper_name: &str, identity: &str) -> bool {
    let binary = helper_binary_path(helper_name);
    let output = Command::new(&binary)
        .arg("erase")
        .arg(identity)
        .output();
    matches!(output, Ok(o) if o.status.success())
}
