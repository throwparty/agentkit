//! Docker's configuration section, typed here and nowhere else.
//!
//! The core configuration holds this section as an untyped value; only this
//! module knows which keys Docker accepts, and a key it does not accept is an
//! error rather than something silently ignored.

use serde::Deserialize;

use super::DockerSettings;
use crate::sandbox::contract::Error;

/// Docker's keys.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DockerConfig {
    /// The image every sandbox runs.
    pub image: Option<String>,
    /// A command to run once after the workspace is materialised.
    #[serde(rename = "setup-command")]
    pub setup_command: Option<String>,
}

impl DockerConfig {
    /// Parse the backend's section from the core's untyped map.
    pub fn from_section(section: &toml::Value) -> Result<Self, Error> {
        section
            .clone()
            .try_into()
            .map_err(|error: toml::de::Error| {
                Error::failed(format!("backends.docker is not valid: {error}"))
            })
    }

    /// The settings a backend needs, or a failure that names what to set.
    pub fn settings(&self) -> Result<DockerSettings, Error> {
        let image = self
            .image
            .as_deref()
            .map(str::trim)
            .filter(|image| !image.is_empty())
            .ok_or_else(|| {
                Error::failed(
                    "backends.docker.image is not set; set it in the user configuration \
                     to the image sandboxes should run",
                )
            })?;

        Ok(DockerSettings {
            image: image.to_string(),
            bind: "127.0.0.1".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::DockerConfig;

    #[test]
    fn settings_carry_the_image_and_default_to_loopback() {
        let config = DockerConfig {
            image: Some("busybox:latest".to_string()),
            setup_command: None,
        };
        let settings = config.settings().expect("settings");
        assert_eq!(settings.image, "busybox:latest");
        assert_eq!(settings.bind, "127.0.0.1");
    }

    #[test]
    fn a_missing_image_names_the_key_and_offers_a_remedy() {
        let error = DockerConfig::default()
            .settings()
            .expect_err("image required");
        let message = error.to_string();
        assert!(message.contains("backends.docker.image"), "{message}");
        assert!(message.contains("user configuration"), "{message}");
    }

    #[test]
    fn a_blank_image_is_treated_as_missing() {
        let config = DockerConfig {
            image: Some("   ".to_string()),
            setup_command: None,
        };
        assert!(config.settings().is_err());
    }

    #[test]
    fn an_unknown_key_is_rejected_by_name() {
        let section: toml::Value =
            toml::from_str("image = 'busybox'\nhost = 'unix:///tmp/docker.sock'")
                .expect("toml parses");
        let error = DockerConfig::from_section(&section).expect_err("unknown key rejected");
        let message = error.to_string();
        assert!(message.contains("host"), "{message}");
    }

    #[test]
    fn the_section_parses_its_own_keys() {
        let section: toml::Value =
            toml::from_str("image = 'busybox'\nsetup-command = 'echo ready'").expect("toml parses");
        let config = DockerConfig::from_section(&section).expect("section parses");
        assert_eq!(config.image.as_deref(), Some("busybox"));
        assert_eq!(config.setup_command.as_deref(), Some("echo ready"));
    }
}
