//! Git's configuration section, typed here and nowhere else.
//!
//! The core configuration holds this section as an untyped value; only this
//! module knows which keys Git accepts, and a key it does not accept is an
//! error rather than something silently ignored.

use serde::Deserialize;

use crate::domain::ScmMode;
use crate::sandbox::contract::WorkspaceBinding;

/// Git's keys.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GitVcsConfig {
    /// Whether snapshots go to a branch or a bare clone.
    #[serde(rename = "snapshot-mode", default)]
    pub snapshot_mode: Option<ScmMode>,
    /// How a sandbox receives its workspace.
    #[serde(default)]
    pub binding: Option<WorkspaceBinding>,
}

impl GitVcsConfig {
    /// Parse the section from the core's untyped map.
    pub fn from_section(section: &toml::Value) -> Result<Self, String> {
        if !section.is_table() {
            return Err("vcs.git must be a table".to_string());
        }
        section
            .clone()
            .try_into()
            .map_err(|error: toml::de::Error| format!("vcs.git is not valid: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::GitVcsConfig;
    use crate::domain::ScmMode;
    use crate::sandbox::contract::WorkspaceBinding;

    fn section(text: &str) -> toml::Value {
        toml::from_str(text).expect("section")
    }

    #[test]
    fn the_snapshot_mode_parses() {
        let config =
            GitVcsConfig::from_section(&section("snapshot-mode = 'direct'")).expect("config");
        assert_eq!(config.snapshot_mode, Some(ScmMode::Direct));
    }

    #[test]
    fn the_binding_parses_and_an_absent_one_is_none() {
        let config = GitVcsConfig::from_section(&section("binding = 'bound'")).expect("config");
        assert_eq!(config.binding, Some(WorkspaceBinding::Bound));

        let config = GitVcsConfig::from_section(&section("binding = 'copied'")).expect("config");
        assert_eq!(config.binding, Some(WorkspaceBinding::Copied));

        let config =
            GitVcsConfig::from_section(&toml::Value::Table(Default::default())).expect("config");
        assert_eq!(config.binding, None, "an absent binding is not a default");
    }

    #[test]
    fn an_unknown_binding_is_refused_naming_the_values() {
        let error =
            GitVcsConfig::from_section(&section("binding = 'hardlink'")).expect_err("unknown");
        assert!(error.contains("vcs.git"), "{error}");
        assert!(
            error.contains("copied") && error.contains("bound"),
            "{error}"
        );
    }

    #[test]
    fn an_absent_mode_stays_unset() {
        let config =
            GitVcsConfig::from_section(&toml::Value::Table(Default::default())).expect("config");
        assert_eq!(config.snapshot_mode, None);
    }

    #[test]
    fn an_unknown_key_is_refused() {
        let error =
            GitVcsConfig::from_section(&section("image = 'busybox'")).expect_err("unknown key");
        assert!(error.contains("vcs.git"), "{error}");
    }
}
