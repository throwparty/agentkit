//! The consumer side of the credential-helper protocol.
//!
//! A helper is a binary named `agentkit-credential-<name>`, resolved as a
//! sibling of the running executable first and then from `PATH`. It is
//! invoked as `agentkit-credential-<name> <command> <component> <identity>`,
//! with an optional JSON body on stdin, and answers on stdout with JSON
//! carrying at least `access_token`. This module is the shared client for
//! that protocol: the helper binaries in this crate are its server side.
//!
//! Nothing here is component-specific: the component (the credential
//! namespace, e.g. `tackle` or `switchboard`) is always a parameter.

use std::path::PathBuf;
use std::process::{Command, Output};

/// The `{cred:IDENTITY}` marker prefix: a whole value of this form resolves
/// through the credential helper.
pub const MARKER_PREFIX: &str = "{cred:";

/// The helper binary name for a helper name: `agentkit-credential-<name>`.
pub fn helper_binary_name(helper: &str) -> String {
    format!("agentkit-credential-{helper}")
}

/// Resolves a credential-helper binary: a sibling of the running
/// executable first, then the first match on `PATH`. The sibling lookup
/// lets a bundled helper next to the binary work without a `PATH` entry.
pub fn resolve_helper_path(helper: &str) -> Option<PathBuf> {
    let name = helper_binary_name(helper);
    if let Ok(exe) = std::env::current_exe() {
        if let Some(sibling) = exe.parent().map(|dir| dir.join(&name)) {
            if sibling.is_file() {
                return Some(sibling);
            }
        }
    }
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(&name))
        .find(|candidate| candidate.is_file())
}

/// The identity in a whole-value `{cred:IDENTITY}` marker, if `value` is
/// exactly one. A marker that is not the entire value, or that carries an
/// empty identity, yields `None`.
pub fn parse_marker(value: &str) -> Option<&str> {
    let rest = value.strip_prefix(MARKER_PREFIX)?;
    let identity = rest.strip_suffix('}')?;
    (!identity.is_empty()).then_some(identity)
}

/// The `PATH` entries, one per line, for a not-found diagnostic.
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

/// Runs the helper: `agentkit-credential-<helper> <command> <component>
/// <identity>`, writing `stdin` when present. The failure strings name the
/// helper and, when it cannot be found, the searched `PATH`.
pub fn run(
    helper: &str,
    command: &str,
    component: &str,
    identity: &str,
    stdin: Option<&str>,
) -> Result<Output, String> {
    let path = resolve_helper_path(helper).ok_or_else(|| {
        format!(
            "credential helper 'agentkit-credential-{helper}' not found.\n  Searched PATH:\n  {}\n  (target: agentkit-credential-{helper})",
            format_path_for_display(),
        )
    })?;

    let mut cmd = Command::new(&path);
    cmd.arg(command);
    cmd.arg(component);
    cmd.arg(identity);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    if stdin.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot spawn helper '{}': {e}", path.display()))?;

    if let Some(data) = stdin {
        if let Some(mut handle) = child.stdin.take() {
            use std::io::Write;
            handle
                .write_all(data.as_bytes())
                .map_err(|e| format!("cannot write to helper '{}': {e}", path.display()))?;
        }
    }

    child.wait_with_output().map_err(|e| {
        format!(
            "helper '{}' wait failed for '{identity}': {e}",
            path.display()
        )
    })
}

/// A credential as returned by the helper's `get`: `access_token` plus the
/// optional OAuth fields.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Credential {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// RFC 3339, unparsed: consumers parse it into their own time type.
    pub expires_at: Option<String>,
    pub account_id: Option<String>,
}

impl Credential {
    /// Parses the helper's JSON stdout. A missing `access_token` is an
    /// error; the optional fields are absent when not present or not
    /// strings.
    pub fn parse(json: &str) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(json)
            .map_err(|err| format!("unparseable helper output: {err}"))?;
        let access_token = value
            .get("access_token")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "helper output carried no access_token".to_owned())?
            .to_owned();
        Ok(Self {
            access_token,
            refresh_token: string_field(&value, "refresh_token"),
            expires_at: string_field(&value, "expires_at"),
            account_id: string_field(&value, "account_id"),
        })
    }
}

fn string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_only_match_the_whole_value() {
        assert_eq!(parse_marker("{cred:github}"), Some("github"));
        assert_eq!(parse_marker("Bearer {cred:github}"), None);
        assert_eq!(parse_marker("{cred:}"), None);
        assert_eq!(parse_marker("{cred:github"), None);
        assert_eq!(parse_marker("plain"), None);
    }

    #[test]
    fn credential_parses_access_token_and_optional_fields() {
        let credential = Credential::parse(
            r#"{"access_token":"tok","refresh_token":"ref","expires_at":"2026-12-31T23:59:59Z","account_id":"acct"}"#,
        )
        .unwrap();
        assert_eq!(credential.access_token, "tok");
        assert_eq!(credential.refresh_token.as_deref(), Some("ref"));
        assert_eq!(
            credential.expires_at.as_deref(),
            Some("2026-12-31T23:59:59Z")
        );
        assert_eq!(credential.account_id.as_deref(), Some("acct"));
    }

    #[test]
    fn credential_requires_an_access_token() {
        assert!(Credential::parse(r#"{"refresh_token":"ref"}"#).is_err());
        assert!(Credential::parse("not json").is_err());
    }

    #[test]
    fn helper_binary_name_is_prefixed() {
        assert_eq!(
            helper_binary_name("keychain"),
            "agentkit-credential-keychain"
        );
    }
}
