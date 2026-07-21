use crate::credential::{CredentialSource, OAuthState, ResolvedCredential};
use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::process::{Command, Output};

fn helper_binary_name(helper_name: &str) -> String {
    format!("agentkit-credential-{helper_name}")
}

fn resolve_helper_path(helper_name: &str) -> Option<PathBuf> {
    let name = helper_binary_name(helper_name);

    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.parent()?.join(&name);
        if sibling.is_file() {
            return Some(sibling);
        }
    }

    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(&name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

pub fn format_path_for_display() -> String {
    match std::env::var("PATH") {
        Ok(path) => {
            let entries: Vec<String> = std::env::split_paths(&path)
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            entries.join("\n  ")
        }
        Err(_) => "(PATH not set)".to_string(),
    }
}

pub fn run_helper_raw(
    helper_name: &str,
    command: &str,
    component: &str,
    identity: &str,
    stdin_data: Option<&str>,
) -> Result<Output, String> {
    let path = resolve_helper_path(helper_name).ok_or_else(|| {
        format!("credential helper 'agentkit-credential-{helper_name}' not found")
    })?;

    let mut cmd = Command::new(&path);
    cmd.arg(command);
    cmd.arg(component);
    cmd.arg(identity);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    if stdin_data.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot spawn helper: {e}"))?;

    if let Some(data) = stdin_data {
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin
                .write_all(data.as_bytes())
                .map_err(|e| format!("cannot write to helper: {e}"))?;
        }
    }

    child
        .wait_with_output()
        .map_err(|e| format!("helper wait failed: {e}"))
}

fn run_helper(
    helper_name: &str,
    command: &str,
    identity: &str,
    stdin_data: Option<&str>,
) -> Result<Output, String> {
    let path = resolve_helper_path(helper_name).ok_or_else(|| {
        format!("credential helper 'agentkit-credential-{helper_name}' not found")
    })?;

    let mut cmd = Command::new(&path);
    cmd.arg(command).arg(identity);
    cmd.stdout(std::process::Stdio::piped());

    if stdin_data.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot spawn helper: {e}"))?;

    if let Some(data) = stdin_data {
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin
                .write_all(data.as_bytes())
                .map_err(|e| format!("cannot write to helper: {e}"))?;
        }
    }

    child
        .wait_with_output()
        .map_err(|e| format!("helper wait failed: {e}"))
}

pub fn parse_credential_json(json: &str, source: CredentialSource) -> Option<ResolvedCredential> {
    let parsed: serde_json::Value = serde_json::from_str(json).ok()?;
    let access_token = parsed.get("access_token")?.as_str()?.to_string();
    let refresh_token = parsed
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let expires_at = parsed
        .get("expires_at")
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));
    let account_id = parsed
        .get("account_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Some(ResolvedCredential {
        value: access_token,
        source,
        oauth: Some(OAuthState {
            refresh_token,
            expires_at,
            account_id,
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
        eprintln!(
            "[helper-get] helper exited with code {}: {stderr}",
            output.status
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

pub fn put(helper_name: &str, identity: &str, credential: &ResolvedCredential) -> bool {
    let json = serde_json::json!({
        "access_token": credential.value,
        "refresh_token": credential.oauth.as_ref().and_then(|o| o.refresh_token.as_deref()),
        "expires_at": credential.oauth.as_ref()
            .and_then(|o| o.expires_at)
            .map(|dt| dt.to_rfc3339()),
        "account_id": credential.oauth.as_ref().and_then(|o| o.account_id.as_deref()),
    });
    let body = serde_json::to_string(&json).unwrap_or_default();
    run_helper(helper_name, "put", identity, Some(&body))
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn delete(helper_name: &str, identity: &str) -> bool {
    run_helper(helper_name, "delete", identity, None)
        .map(|o| o.status.success())
        .unwrap_or(false)
}
