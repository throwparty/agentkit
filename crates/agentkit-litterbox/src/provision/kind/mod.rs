//! The compiled-in provisioning kinds.
//!
//! A kind is a name plus the settings it owns. The contract carries the step
//! generically; this module is the only place that knows what a kind accepts
//! and, later, what preparing it means.

pub mod command;
pub mod devshell;
pub mod registry;

use std::collections::{BTreeMap, HashMap};

use crate::sandbox::contract::{ContributionSort, Placement, ProvisionStep};

/// The settings a kind accepts, as they arrived from configuration.
pub type Settings = BTreeMap<String, toml::Value>;

/// What a kind is, without parsing its settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KindDescription {
    /// The name configuration and surfaces use.
    pub name: &'static str,
    /// Where a step of this kind runs.
    pub placement: Placement,
    /// The contribution sorts the kind produces.
    pub sorts: &'static [ContributionSort],
    /// Whether the step evaluates or runs project code and needs consent.
    pub requires_consent: bool,
    /// The most steps of this kind one backend sequence may name.
    pub max_per_sequence: Option<usize>,
}

/// Why a kind's settings are unusable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsError {
    /// What is wrong.
    pub reason: String,
    /// What the user can do about it.
    pub remedy: String,
}

impl SettingsError {
    /// A settings error with a reason and a remedy.
    pub fn new(reason: impl Into<String>, remedy: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            remedy: remedy.into(),
        }
    }
}

/// One provisioning kind.
pub trait Kind: Send + Sync {
    /// What this kind is.
    fn describe(&self) -> KindDescription;

    /// Validate the settings the step carries.
    fn validate(&self, settings: &Settings) -> Result<(), SettingsError>;
}

/// Refuse a setting this kind does not define.
pub(crate) fn reject_unknown(settings: &Settings, allowed: &[&str]) -> Result<(), SettingsError> {
    for key in settings.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(SettingsError::new(
                format!("unknown setting '{key}'"),
                format!("remove it; this kind accepts {}", allowed.join(", ")),
            ));
        }
    }
    Ok(())
}

/// Read a required string setting.
pub(crate) fn required_string(settings: &Settings, key: &str) -> Result<String, SettingsError> {
    let value = settings
        .get(key)
        .and_then(toml::Value::as_str)
        .ok_or_else(|| {
            SettingsError::new(
                format!("missing required setting '{key}'"),
                format!("set {key} to a non-empty string"),
            )
        })?;
    if value.trim().is_empty() {
        return Err(SettingsError::new(
            format!("setting '{key}' is empty"),
            format!("set {key} to a non-empty string"),
        ));
    }
    Ok(value.to_string())
}

/// Validate one step and return its kind.
pub fn validate(step: &ProvisionStep) -> Result<&'static dyn Kind, SettingsError> {
    let kind = registry::lookup(&step.kind).ok_or_else(|| {
        SettingsError::new(
            format!("unknown provisioning kind '{}'", step.kind),
            "use a kind this build supports",
        )
    })?;
    kind.validate(&step.settings)?;
    Ok(kind)
}

/// Validate a backend's sequence, including kinds that may appear once.
pub fn validate_sequence(backend: &str, steps: &[&ProvisionStep]) -> Result<(), SettingsError> {
    let mut first: HashMap<&'static str, &str> = HashMap::new();
    for step in steps {
        let description = validate(step)?.describe();
        if description.max_per_sequence == Some(1)
            && let Some(other) = first.insert(description.name, &step.identity)
        {
            return Err(SettingsError::new(
                format!(
                    "backend '{backend}' names two {} steps, '{other}' and '{}'",
                    description.name, step.identity
                ),
                "keep one step of this kind per backend sequence",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(identity: &str, kind: &str, settings: &[(&str, &str)]) -> ProvisionStep {
        ProvisionStep {
            identity: identity.to_string(),
            kind: kind.to_string(),
            settings: settings
                .iter()
                .map(|(key, value)| (key.to_string(), toml::Value::String(value.to_string())))
                .collect(),
        }
    }

    #[test]
    fn a_command_step_parses() {
        assert!(validate(&step("fetch", "command", &[("run", "true")])).is_ok());
    }

    #[test]
    fn a_devshell_step_parses() {
        assert!(validate(&step("dev", "devshell", &[("installable", "./nix")])).is_ok());
    }

    #[test]
    fn an_unknown_kind_is_refused() {
        let error = validate(&step("x", "nix", &[])).expect_err("unknown kind");
        assert!(error.reason.contains("nix"), "{error:?}");
    }

    #[test]
    fn an_unknown_setting_is_refused() {
        let error = validate(&step(
            "dev",
            "devshell",
            &[("installable", "./nix"), ("extra", "1")],
        ))
        .expect_err("unknown setting");
        assert!(error.reason.contains("extra"), "{error:?}");
    }

    #[test]
    fn a_missing_installable_is_refused() {
        let error = validate(&step("dev", "devshell", &[])).expect_err("missing installable");
        assert!(error.reason.contains("installable"), "{error:?}");
    }

    #[test]
    fn two_devshell_steps_are_refused() {
        let first = step("dev", "devshell", &[("installable", "./nix")]);
        let second = step("dev2", "devshell", &[("installable", "./other")]);
        let error =
            validate_sequence("docker", &[&first, &second]).expect_err("two devshell steps");
        assert!(error.reason.contains("dev"), "{error:?}");
    }

    #[test]
    fn the_test_kind_is_registered() {
        assert!(registry::lookup("test-only").is_some());
    }
}
