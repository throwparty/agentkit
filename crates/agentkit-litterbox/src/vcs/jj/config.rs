//! The `[vcs.jj]` section, typed here and nowhere else.
//!
//! The core configuration holds this section as an untyped value; only this
//! module knows which keys jj accepts. It has no `snapshot-mode`: jj is
//! remote-only, so a mode is an unknown key rather than something ignored.

use serde::Deserialize;

use crate::sandbox::contract::WorkspaceBinding;

/// jj's keys.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct JjVcsConfig {
    /// How a sandbox receives its workspace.
    #[serde(default)]
    pub binding: Option<WorkspaceBinding>,
    /// Configuration applied to Litterbox's clone. TOML.
    #[serde(default)]
    pub config: Option<String>,
}

impl JjVcsConfig {
    /// Parse the section from the core's untyped map.
    pub fn from_section(section: &toml::Value) -> Result<Self, String> {
        if !section.is_table() {
            return Err("vcs.jj must be a table".to_string());
        }
        section
            .clone()
            .try_into()
            .map_err(|error: toml::de::Error| format!("vcs.jj is not valid: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::JjVcsConfig;
    use crate::sandbox::contract::WorkspaceBinding;

    fn section(text: &str) -> toml::Value {
        toml::from_str(text).expect("section")
    }

    #[test]
    fn the_binding_parses_and_an_absent_one_is_none() {
        let config = JjVcsConfig::from_section(&section("binding = 'bound'")).expect("config");
        assert_eq!(config.binding, Some(WorkspaceBinding::Bound));

        let config =
            JjVcsConfig::from_section(&toml::Value::Table(Default::default())).expect("config");
        assert_eq!(config.binding, None, "an absent binding is not a default");
    }

    #[test]
    fn the_config_string_parses() {
        let config = JjVcsConfig::from_section(&section("config = 'x'")).expect("config");
        assert_eq!(config.config.as_deref(), Some("x"));
    }

    #[test]
    fn a_snapshot_mode_is_refused() {
        let error = JjVcsConfig::from_section(&section("snapshot-mode = 'direct'"))
            .expect_err("jj has no mode");
        assert!(error.contains("vcs.jj"), "{error}");
        assert!(error.contains("snapshot-mode"), "{error}");
    }

    #[test]
    fn an_unknown_key_is_refused() {
        let error =
            JjVcsConfig::from_section(&section("image = 'busybox'")).expect_err("unknown key");
        assert!(error.contains("vcs.jj"), "{error}");
    }
}
