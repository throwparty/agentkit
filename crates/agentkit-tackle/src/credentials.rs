//! Tackle credential policy: the configured helper name, the `tackle` component, and the resolve wrappers over the shared `agentkit-credentials` consumer.

use crate::config::Config;

/// The credential component every tackle credential rides under: the
/// helper protocol is `agentkit-credential-{helper} get {component}
/// {identity}`, and the component is this constant.
pub const CREDENTIAL_COMPONENT: &str = "tackle";

/// The effective credential helper when `credential_helper` is unset:
/// the helper documented as the default.
pub const DEFAULT_CREDENTIAL_HELPER: &str = "keychain";

/// The effective credential helper for a configuration: the
/// `credential_helper` value, or [`DEFAULT_CREDENTIAL_HELPER`] when
/// unset.
pub fn credential_helper_name(config: &Config) -> String {
    config
        .credential_helper
        .clone()
        .unwrap_or_else(|| DEFAULT_CREDENTIAL_HELPER.to_owned())
}

/// Why a credential-helper invocation failed.
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    /// The helper command failed: missing binary, non-zero exit, or
    /// unparseable stdout.
    #[error("credential helper agentkit-credential-{helper} failed for {identity}: {reason}")]
    Helper {
        helper: String,
        identity: String,
        reason: String,
    },
    /// The helper's JSON stdout carried no `access_token`.
    #[error(
        "credential helper agentkit-credential-{helper} returned no access_token for {identity}"
    )]
    MissingToken { helper: String, identity: String },
}

/// Resolves a credential through the helper command: `agentkit-credential-
/// {helper} get {CREDENTIAL_COMPONENT} {identity}` — the credential-helper
/// protocol with [`CREDENTIAL_COMPONENT`] as the component. The helper's
/// stdout is JSON carrying `access_token`.
pub fn resolve_credential_value(helper: &str, identity: &str) -> Result<String, CredentialError> {
    let output =
        agentkit_credentials::consumer::run(helper, "get", CREDENTIAL_COMPONENT, identity, None)
            .map_err(|reason| CredentialError::Helper {
                helper: helper.to_owned(),
                identity: identity.to_owned(),
                reason,
            })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let reason = if stderr.is_empty() {
            output.status.to_string()
        } else {
            stderr
        };
        return Err(CredentialError::Helper {
            helper: helper.to_owned(),
            identity: identity.to_owned(),
            reason,
        });
    }
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|err| CredentialError::Helper {
            helper: helper.to_owned(),
            identity: identity.to_owned(),
            reason: format!("unparseable stdout: {err}"),
        })?;
    value
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| CredentialError::MissingToken {
            helper: helper.to_owned(),
            identity: identity.to_owned(),
        })
}

/// Resolves a credential for a caller that only needs the secret: the
/// value form, with failures logged at warn and mapped to `None`.
pub fn resolve_credential(helper: &str, identity: &str) -> Option<secrecy::SecretString> {
    match resolve_credential_value(helper, identity) {
        Ok(token) => Some(secrecy::SecretString::new(token.into())),
        Err(err) => {
            tracing::warn!("{err}");
            None
        }
    }
}

/// Tests that mutate the process-global PATH must serialize: two
/// concurrent read-modify-write races drop one test's entry. Shared
/// with the MCP spawn-env tests.
#[cfg(test)]
pub(crate) static PATH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret as _;
    use std::io::Write as _;

    fn write_fake_helper(dir: &std::path::Path, body: &str) {
        let path = dir.join("agentkit-credential-test");
        let mut file = std::fs::File::create(&path).unwrap();
        // The real helpers reject an invocation without the component
        // argument; this fake does too, so a regression cannot pass.
        writeln!(
            file,
            "#!/bin/sh\n\
             [ \"$#\" -eq 3 ] && [ \"$1\" = get ] && [ \"$2\" = tackle ] || exit 3\n\
             echo '{body}'"
        )
        .unwrap();
        file.flush().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn effective_helper_defaults_to_keychain() {
        assert_eq!(
            credential_helper_name(&Config::default()),
            DEFAULT_CREDENTIAL_HELPER,
        );
        let config = Config {
            credential_helper: Some("file".into()),
            ..Config::default()
        };
        assert_eq!(credential_helper_name(&config), "file");
    }

    #[test]
    fn credential_resolution_reads_the_helper_stdout() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_helper(dir.path(), "{\"access_token\": \"secret\"}");
        let _guard = PATH_LOCK.lock().unwrap();
        let path_var = std::env::var("PATH").unwrap();
        std::env::set_var("PATH", format!("{}:{}", dir.path().display(), path_var));
        let credential = resolve_credential("test", "my-endpoint");
        std::env::set_var("PATH", path_var);
        assert_eq!(
            credential.as_ref().map(|secret| secret.expose_secret()),
            Some("secret"),
        );
    }
}
