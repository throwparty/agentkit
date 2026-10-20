//! The `tackle auth` subcommand: store, inspect, and remove the bearer
//! tokens Tackle resolves through the credential helper.
//!
//! Every credential rides under the helper's `tackle` component, keyed
//! by identity — an endpoint name or an MCP `{cred:IDENTITY}` marker.
//! The commands write through the same helper the running server reads,
//! so a stored token is immediately usable.

use crate::config::{Auth as EndpointAuth, Config, McpServerConfig};
use crate::credentials::{credential_helper_name, resolve_credential_value, CREDENTIAL_COMPONENT};
use agentkit_credentials::consumer::{helper_binary_name, resolve_helper_path};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::process::{Command, Output, Stdio};

/// A parsed `tackle auth` invocation.
pub enum AuthCommand {
    Add { identity: String, value: String },
    Status { identity: Option<String> },
    Token { identity: String },
    Logout { identity: String },
}

/// Executes an auth command against the configured credential helper.
/// `credential_helper` overrides the configured helper when present.
pub fn handle_auth(
    command: AuthCommand,
    config: &Config,
    credential_helper: Option<&str>,
) -> Result<String, String> {
    let helper = credential_helper
        .map(str::to_owned)
        .unwrap_or_else(|| credential_helper_name(config));
    match command {
        AuthCommand::Add { identity, value } => add(&helper, &identity, &value),
        AuthCommand::Status { identity } => status(config, &helper, identity.as_deref()),
        AuthCommand::Token { identity } => token(&helper, &identity),
        AuthCommand::Logout { identity } => logout(&helper, &identity),
    }
}

fn helper_binary(helper: &str) -> String {
    helper_binary_name(helper)
}

/// Runs the helper for one operation, writing `stdin_data` when present.
fn run_helper(
    helper: &str,
    operation: &str,
    identity: &str,
    stdin_data: Option<&str>,
) -> Result<Output, String> {
    let binary = helper_binary(helper);
    let path = resolve_helper_path(helper)
        .ok_or_else(|| format!("credential helper {binary} was not found"))?;
    let mut command = Command::new(path);
    command
        .arg(operation)
        .arg(CREDENTIAL_COMPONENT)
        .arg(identity)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin_data.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command.spawn().map_err(|err| {
        format!("credential helper {binary} is unavailable for '{identity}': {err}")
    })?;
    if let Some(data) = stdin_data {
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(data.as_bytes()).map_err(|err| {
                format!("credential helper {binary}: cannot write the credential for '{identity}': {err}")
            })?;
        }
    }
    child.wait_with_output().map_err(|err| {
        format!("credential helper {binary}: waiting for '{identity}' failed: {err}")
    })
}

/// Fails when the helper exited non-zero, folding its stderr in.
fn check_success(
    helper: &str,
    action: &str,
    identity: &str,
    output: &Output,
) -> Result<(), String> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    let reason = if detail.is_empty() {
        format!("exited with {}", output.status)
    } else {
        format!("exited with {}: {detail}", output.status)
    };
    Err(format!(
        "credential helper {} failed to {action} credential for '{identity}' ({reason})",
        helper_binary(helper)
    ))
}

fn add(helper: &str, identity: &str, value: &str) -> Result<String, String> {
    let body = serde_json::json!({ "access_token": value }).to_string();
    let output = run_helper(helper, "put", identity, Some(&body))?;
    check_success(helper, "store", identity, &output)?;
    Ok(format!(
        "✓ Credential stored for '{identity}' (helper: {}).",
        helper_binary(helper)
    ))
}

fn token(helper: &str, identity: &str) -> Result<String, String> {
    resolve_credential_value(helper, identity).map_err(|err| err.to_string())
}

fn logout(helper: &str, identity: &str) -> Result<String, String> {
    let output = run_helper(helper, "delete", identity, None)?;
    check_success(helper, "erase", identity, &output)?;
    Ok(format!("✓ Credential removed for '{identity}'."))
}

/// Reports each configured endpoint and MCP credential identity with
/// whether its credential resolves.
fn status(config: &Config, helper: &str, filter: Option<&str>) -> Result<String, String> {
    let mut lines: Vec<(String, String)> = Vec::new();

    for (name, endpoint) in &config.endpoints {
        let detail = match endpoint.auth {
            EndpointAuth::None => "auth=none, source=none".to_owned(),
            EndpointAuth::Helper => {
                format!("auth=helper, source={}", source_label(helper, name))
            }
        };
        lines.push((name.clone(), detail));
    }

    for (identity, servers) in mcp_credential_identities(config) {
        let detail = format!(
            "via=mcp_servers.{}, source={}",
            servers.join(",mcp_servers."),
            source_label(helper, &identity),
        );
        lines.push((identity, detail));
    }

    lines.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = format!("Credential helper: {}\n", helper_binary(helper));
    let mut shown = 0;
    for (id, detail) in &lines {
        if let Some(filter) = filter {
            if id != filter {
                continue;
            }
        }
        out.push_str(&format!("{id}: {detail}\n"));
        shown += 1;
    }
    if let Some(filter) = filter {
        if shown == 0 {
            return Err(format!(
                "no configured endpoint or MCP credential named '{filter}'"
            ));
        }
    }
    Ok(out.trim_end().to_owned())
}

/// Whether the identity's credential resolves, labelled with its source.
fn source_label(helper: &str, identity: &str) -> String {
    match resolve_credential_value(helper, identity) {
        Ok(_) => helper_binary(helper),
        Err(_) => "unconfigured".to_owned(),
    }
}

/// The `{cred:IDENTITY}` identities declared by stdio MCP servers,
/// mapped to the servers that declare each one.
fn mcp_credential_identities(config: &Config) -> BTreeMap<String, Vec<String>> {
    let mut identities: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (server, server_config) in &config.mcp_servers {
        let McpServerConfig::Stdio { env, .. } = server_config else {
            continue;
        };
        for value in env.values() {
            if let Some(identity) = crate::mcp::credential_marker_identity(value) {
                identities
                    .entry(identity.to_owned())
                    .or_default()
                    .push(server.clone());
            }
        }
    }
    identities
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EndpointConfig, WireFormat};
    use std::path::Path;

    /// Writes an `agentkit-credential-test` helper that stores each
    /// identity as a file under `store`, implementing the `get`, `put`,
    /// and `delete` commands for component `tackle`.
    fn write_fake_helper(dir: &Path, store: &Path) {
        let path = dir.join("agentkit-credential-test");
        let script = format!(
            "#!/bin/sh\n\
             op=\"$1\"; comp=\"$2\"; id=\"$3\"\n\
             [ \"$comp\" = tackle ] || exit 3\n\
             file=\"{}/$id\"\n\
             case \"$op\" in\n\
             get) [ -f \"$file\" ] && cat \"$file\" || exit 1 ;;\n\
             put) cat > \"$file\" ;;\n\
             delete) rm -f \"$file\" ;;\n\
             *) exit 3 ;;\n\
             esac\n",
            store.display()
        );
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn config() -> Config {
        let mut config = Config {
            credential_helper: Some("test".to_owned()),
            ..Config::default()
        };
        config.endpoints.insert(
            "local".to_owned(),
            EndpointConfig {
                base_url: "http://localhost:11434/v1".to_owned(),
                wire_format: WireFormat::OpenaiChatCompletions,
                auth: EndpointAuth::None,
                models: Vec::new(),
            },
        );
        config.endpoints.insert(
            "rogue".to_owned(),
            EndpointConfig {
                base_url: "https://rogue.example.com".to_owned(),
                wire_format: WireFormat::OpenaiChatCompletions,
                auth: EndpointAuth::Helper,
                models: Vec::new(),
            },
        );
        config.mcp_servers.insert(
            "github".to_owned(),
            McpServerConfig::Stdio {
                command: "github-mcp".to_owned(),
                args: Vec::new(),
                env: BTreeMap::from([(
                    "GITHUB_PERSONAL_ACCESS_TOKEN".to_owned(),
                    "{cred:github}".to_owned(),
                )]),
            },
        );
        config
    }

    /// Runs `body` with a fake helper on `PATH`, serialised against the
    /// other tests that mutate `PATH`.
    fn with_fake_helper(body: impl FnOnce()) {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("store");
        std::fs::create_dir_all(&store).unwrap();
        write_fake_helper(dir.path(), &store);

        let guard = crate::credentials::PATH_LOCK.lock().unwrap();
        let original = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{original}", dir.path().display()));
        body();
        std::env::set_var("PATH", original);
        drop(guard);
    }

    #[test]
    fn add_token_logout_round_trips() {
        with_fake_helper(|| {
            let stored = add("test", "rogue", "tok-123").unwrap();
            assert!(stored.contains("Credential stored for 'rogue'"), "{stored}");
            assert_eq!(token("test", "rogue").unwrap(), "tok-123");

            let removed = logout("test", "rogue").unwrap();
            assert!(
                removed.contains("Credential removed for 'rogue'"),
                "{removed}"
            );
            assert!(token("test", "rogue").is_err());
        });
    }

    #[test]
    fn status_reports_endpoints_and_mcp_markers() {
        with_fake_helper(|| {
            add("test", "github", "ghp").unwrap();
            let out = status(&config(), "test", None).unwrap();
            assert!(
                out.contains("Credential helper: agentkit-credential-test"),
                "{out}"
            );
            assert!(out.contains("local: auth=none, source=none"), "{out}");
            assert!(
                out.contains("rogue: auth=helper, source=unconfigured"),
                "{out}"
            );
            assert!(
                out.contains("github: via=mcp_servers.github, source=agentkit-credential-test"),
                "{out}"
            );
        });
    }

    #[test]
    fn status_filter_keeps_only_the_named_identity() {
        with_fake_helper(|| {
            let out = status(&config(), "test", Some("rogue")).unwrap();
            assert!(out.contains("rogue:"), "{out}");
            assert!(!out.contains("local:"), "{out}");
            assert!(!out.contains("github:"), "{out}");
        });
    }

    #[test]
    fn status_with_an_unknown_filter_is_an_error() {
        let err = status(&Config::default(), "test", Some("nope")).unwrap_err();
        assert!(
            err.contains("no configured endpoint or MCP credential"),
            "{err}"
        );
    }
}
