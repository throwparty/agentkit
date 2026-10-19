use crate::credential::{CredentialSource, OAuthState, ResolvedCredential};
use agentkit_credentials::consumer;
use chrono::{DateTime, Utc};
use std::process::Output;

pub fn format_path_for_display() -> String {
    consumer::format_path_for_display()
}

pub fn run_helper_raw(
    helper_name: &str,
    command: &str,
    component: &str,
    identity: &str,
    stdin_data: Option<&str>,
) -> Result<Output, String> {
    consumer::run(helper_name, command, component, identity, stdin_data)
}

const COMPONENT: &str = "switchboard";

fn run_helper(
    helper_name: &str,
    command: &str,
    identity: &str,
    stdin_data: Option<&str>,
) -> Result<Output, String> {
    run_helper_raw(helper_name, command, COMPONENT, identity, stdin_data)
}

pub fn parse_credential_json(json: &str, source: CredentialSource) -> Option<ResolvedCredential> {
    let credential = consumer::Credential::parse(json).ok()?;
    let expires_at = credential
        .expires_at
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    Some(ResolvedCredential {
        value: credential.access_token,
        source,
        oauth: Some(OAuthState {
            refresh_token: credential.refresh_token,
            expires_at,
            account_id: credential.account_id,
        }),
    })
}

pub fn get(helper_name: &str, identity: &str) -> Option<ResolvedCredential> {
    let output = match run_helper(helper_name, "get", identity, None) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[helper-get] run_helper failed: {e}");
            return None;
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if stderr.trim().is_empty() {
            "no credential stored".to_string()
        } else {
            stderr.trim().to_string()
        };
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| output.status.to_string());
        eprintln!(
            "[helper-get] no credential for '{identity}' (helper 'agentkit-credential-{helper_name}' exited with {code}): {detail}",
        );
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let source = CredentialSource::Helper {
        helper_name: helper_name.to_string(),
    };
    let result = parse_credential_json(&stdout, source);
    if result.is_none() {
        eprintln!("[helper-get] failed to parse credential JSON from helper output");
    }
    result
}

fn check_helper_output(
    helper_name: &str,
    operation: &str,
    identity: &str,
    output: &Output,
) -> Result<(), String> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    let code = output
        .status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| output.status.to_string());
    if detail.is_empty() {
        Err(format!(
            "credential helper 'agentkit-credential-{helper_name}' failed to {operation} credential for '{identity}' (exited with {code}, no stderr)"
        ))
    } else {
        Err(format!(
            "credential helper 'agentkit-credential-{helper_name}' failed to {operation} credential for '{identity}' (exited with {code}): {detail}"
        ))
    }
}

pub fn put(
    helper_name: &str,
    identity: &str,
    credential: &ResolvedCredential,
) -> Result<(), String> {
    let json = serde_json::json!({
        "access_token": credential.value,
        "refresh_token": credential.oauth.as_ref().and_then(|o| o.refresh_token.as_deref()),
        "expires_at": credential.oauth.as_ref()
            .and_then(|o| o.expires_at)
            .map(|dt| dt.to_rfc3339()),
        "account_id": credential.oauth.as_ref().and_then(|o| o.account_id.as_deref()),
    });
    let body = serde_json::to_string(&json).unwrap_or_default();
    let output = run_helper(helper_name, "put", identity, Some(&body))?;
    check_helper_output(helper_name, "store", identity, &output)
}

pub fn delete(helper_name: &str, identity: &str) -> Result<(), String> {
    let output = run_helper(helper_name, "delete", identity, None)?;
    check_helper_output(helper_name, "erase", identity, &output)
}
