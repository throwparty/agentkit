//! Stdio spawn hygiene: explicit argv (no shell), only the named environment entries, and `{cred:IDENTITY}` resolution at spawn.

use std::collections::BTreeMap;
use tokio::process::Command;

/// The spawn-hygiene operations, abstracted so construction is
/// unit-testable against a recording fake.
pub trait ConfigureCommand {
    fn set_program(&mut self, program: &str);
    fn set_args(&mut self, args: &[String]);
    fn clear_env(&mut self);
    fn set_env(&mut self, key: &str, value: &str);
}

impl ConfigureCommand for Command {
    /// Only `Command::new` can set the program, so this replaces the
    /// command; `configure_stdio_spawn` calls it before anything else.
    fn set_program(&mut self, program: &str) {
        *self = Command::new(program);
    }

    fn set_args(&mut self, args: &[String]) {
        self.args(args);
    }

    fn clear_env(&mut self) {
        self.env_clear();
    }

    fn set_env(&mut self, key: &str, value: &str) {
        self.env(key, value);
    }
}

/// An env value failed to resolve for a stdio spawn.
#[derive(Debug, thiserror::Error)]
pub enum EnvValueError {
    /// A value containing the marker was not the marker alone.
    #[error("env `{key}`: a `{{cred:...}}` marker must be the entire value")]
    MarkerNotEntire { key: String },
    /// The marker carried no identity.
    #[error("env `{key}`: the `{{cred:...}}` marker has an empty identity")]
    EmptyIdentity { key: String },
    /// The credential helper failed to produce the secret.
    #[error("env `{key}`: {source}")]
    Credential {
        key: String,
        #[source]
        source: crate::credentials::CredentialError,
    },
}

/// The identity in a whole-value `{cred:IDENTITY}` marker, if `value`
/// is exactly one. A marker that is not the entire value, or that
/// carries an empty identity, yields `None` here; [`resolve_env_value`]
/// reports those as errors at spawn.
pub fn credential_marker_identity(value: &str) -> Option<&str> {
    agentkit_credentials::consumer::parse_marker(value)
}

/// Resolves one env value for a spawn: a value that is the entire
/// `{cred:IDENTITY}` marker resolves through `credential_helper`
/// (component [`crate::credentials::CREDENTIAL_COMPONENT`]); any
/// value without the marker passes through verbatim. Resolution runs
/// at spawn, never at config load, so secrets never enter `Config`.
fn resolve_env_value(
    credential_helper: &str,
    key: &str,
    value: &str,
) -> Result<String, EnvValueError> {
    if !value.contains(agentkit_credentials::consumer::MARKER_PREFIX) {
        return Ok(value.to_owned());
    }
    let Some(rest) = value.strip_prefix(agentkit_credentials::consumer::MARKER_PREFIX) else {
        return Err(EnvValueError::MarkerNotEntire {
            key: key.to_owned(),
        });
    };
    let Some(identity) = rest.strip_suffix('}') else {
        return Err(EnvValueError::MarkerNotEntire {
            key: key.to_owned(),
        });
    };
    if identity.is_empty() {
        return Err(EnvValueError::EmptyIdentity {
            key: key.to_owned(),
        });
    }
    crate::credentials::resolve_credential_value(credential_helper, identity).map_err(|source| {
        EnvValueError::Credential {
            key: key.to_owned(),
            source,
        }
    })
}

/// Applies spawn hygiene to a stdio server spawn: explicit argv (no
/// shell interpretation) and only the named environment entries —
/// no blanket forwarding of tackle's own environment. Env values
/// resolve first: a failed `{cred:IDENTITY}` resolution returns
/// before the command is mutated at all.
pub fn configure_stdio_spawn(
    cmd: &mut impl ConfigureCommand,
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    credential_helper: &str,
) -> Result<(), EnvValueError> {
    let resolved = env
        .iter()
        .map(|(key, value)| {
            resolve_env_value(credential_helper, key, value).map(|value| (key, value))
        })
        .collect::<Result<Vec<_>, _>>()?;
    cmd.set_program(command);
    cmd.set_args(args);
    cmd.clear_env();
    for (key, value) in resolved {
        cmd.set_env(key, &value);
    }
    Ok(())
}
