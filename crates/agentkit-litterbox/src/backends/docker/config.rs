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
    /// Memory ceiling per sandbox, in MiB.
    #[serde(rename = "memory-mb")]
    pub memory_mb: Option<u64>,
    /// CPU cores per sandbox.
    pub cpus: Option<u32>,
    /// Maximum tasks or processes per sandbox.
    #[serde(rename = "pids-limit")]
    pub pids_limit: Option<u32>,
    /// Extra Linux capabilities to grant, such as CHOWN or SETUID.
    #[serde(rename = "cap-add", default)]
    pub cap_add: Vec<String>,
}

impl DockerConfig {
    /// Parse the backend's section from the core's untyped map.
    pub fn from_section(section: &toml::Value) -> Result<Self, Error> {
        if section.get("setup-command").is_some() {
            return Err(Error::Misconfigured {
                message: "setup-command is not a backend setting; declare a [[provision]] \
                          step with kind = 'command' and list it in [backends.<name>].provision"
                    .to_string(),
            });
        }

        section
            .clone()
            .try_into()
            .map_err(|error: toml::de::Error| Error::Misconfigured {
                message: format!("backends.docker is not valid: {error}"),
            })
    }

    /// The settings a backend needs.
    ///
    /// A missing image is not rejected here so introspection can still report
    /// it as a structured unavailable check; create refuses it explicitly.
    pub fn settings(&self) -> Result<DockerSettings, Error> {
        let image = self
            .image
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .to_string();
        if !image.is_empty() && !image.contains("@sha256:") {
            tracing::warn!(
                image = %image,
                "the sandbox image is pinned by a mutable tag; prefer name@sha256:..."
            );
        }

        let defaults = DockerSettings::default();
        let memory_mb = self.memory_mb.unwrap_or(defaults.memory_mb);
        // Docker refuses anything below 6 MiB; zero means "no limit", which
        // would silently remove the bound the user asked for.
        if memory_mb < 6 {
            return Err(Error::Misconfigured {
                message: format!(
                    "backends.docker.memory-mb is {memory_mb}; it must be at least 6 MiB"
                ),
            });
        }

        let cpus = self.cpus.unwrap_or(defaults.cpus);
        if cpus == 0 {
            return Err(Error::Misconfigured {
                message: "backends.docker.cpus is 0; it must be at least 1".to_string(),
            });
        }

        let pids_limit = self.pids_limit.unwrap_or(defaults.pids_limit);
        if pids_limit == 0 {
            return Err(Error::Misconfigured {
                message: "backends.docker.pids-limit is 0; it must be at least 1".to_string(),
            });
        }

        for capability in &self.cap_add {
            let valid = !capability.is_empty()
                && capability.chars().all(|character| {
                    character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
                });
            if !valid {
                return Err(Error::Misconfigured {
                    message: format!(
                        "backends.docker.cap-add entry '{capability}' is not a capability name"
                    ),
                });
            }
            if capability == "ALL" {
                return Err(Error::Misconfigured {
                    message: "backends.docker.cap-add cannot grant ALL; name the capabilities the \
                              image genuinely needs"
                        .to_string(),
                });
            }
            if matches!(
                capability.as_str(),
                "SYS_ADMIN"
                    | "SYS_PTRACE"
                    | "SYS_MODULE"
                    | "DAC_READ_SEARCH"
                    | "DAC_OVERRIDE"
                    | "NET_ADMIN"
            ) {
                tracing::warn!(
                    capability = %capability,
                    "backends.docker.cap-add grants a capability that widens the sandbox escape surface"
                );
            }
        }

        Ok(DockerSettings {
            image,
            memory_mb,
            cpus,
            pids_limit,
            cap_add: self.cap_add.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::DockerConfig;

    #[test]
    fn settings_carry_the_image() {
        let config = DockerConfig {
            image: Some("busybox:latest".to_string()),
            ..DockerConfig::default()
        };
        let settings = config.settings().expect("settings");
        assert_eq!(settings.image, "busybox:latest");
    }

    #[test]
    fn the_defaults_bound_memory_and_cpu() {
        let config = DockerConfig {
            image: Some("busybox:latest".to_string()),
            ..DockerConfig::default()
        };
        let settings = config.settings().expect("settings");
        assert_eq!(settings.memory_mb, 4096);
        assert_eq!(settings.cpus, 2);
        assert_eq!(settings.pids_limit, 512);
        assert!(settings.cap_add.is_empty());
    }

    #[test]
    fn the_section_can_raise_the_bounds() {
        let config = DockerConfig::from_section(
            &toml::from_str(
                "image = 'busybox'\nmemory-mb = 8192\ncpus = 4\npids-limit = 256\n\
                 cap-add = ['CHOWN', 'SETUID']",
            )
            .expect("section"),
        )
        .expect("config");
        let settings = config.settings().expect("settings");
        assert_eq!(settings.memory_mb, 8192);
        assert_eq!(settings.cpus, 4);
        assert_eq!(settings.pids_limit, 256);
        assert_eq!(settings.cap_add, vec!["CHOWN", "SETUID"]);
    }

    #[test]
    fn a_zero_bound_is_refused_rather_than_unbounded() {
        let memory = DockerConfig {
            image: Some("busybox:latest".to_string()),
            memory_mb: Some(0),
            ..DockerConfig::default()
        };
        let message = memory
            .settings()
            .expect_err("zero memory refused")
            .to_string();
        assert!(message.contains("memory-mb"), "{message}");

        let cpus = DockerConfig {
            image: Some("busybox:latest".to_string()),
            cpus: Some(0),
            ..DockerConfig::default()
        };
        let message = cpus.settings().expect_err("zero cpus refused").to_string();
        assert!(message.contains("cpus"), "{message}");

        let pids = DockerConfig {
            image: Some("busybox:latest".to_string()),
            pids_limit: Some(0),
            ..DockerConfig::default()
        };
        let message = pids.settings().expect_err("zero pids refused").to_string();
        assert!(message.contains("pids-limit"), "{message}");
    }

    #[test]
    fn a_capability_name_is_validated() {
        let config = DockerConfig {
            image: Some("busybox:latest".to_string()),
            cap_add: vec!["chown".to_string()],
            ..DockerConfig::default()
        };
        let message = config
            .settings()
            .expect_err("lowercase is not a capability name")
            .to_string();
        assert!(message.contains("cap-add"), "{message}");
    }

    #[test]
    fn granting_every_capability_is_refused() {
        let config = DockerConfig {
            image: Some("busybox:latest".to_string()),
            cap_add: vec!["ALL".to_string()],
            ..DockerConfig::default()
        };
        let message = config
            .settings()
            .expect_err("ALL is not a capability grant")
            .to_string();
        assert!(message.contains("cap-add"), "{message}");
        assert!(message.contains("ALL"), "{message}");
    }

    #[test]
    fn a_missing_or_blank_image_is_reported_as_empty_settings() {
        for config in [
            DockerConfig::default(),
            DockerConfig {
                image: Some("   ".to_string()),
                ..DockerConfig::default()
            },
        ] {
            let settings = config.settings().expect("settings");
            assert!(settings.image.is_empty());
        }
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
    fn a_backend_setup_command_points_at_the_provision_section() {
        let section: toml::Value =
            toml::from_str("image = 'busybox'\nsetup-command = 'echo ready'").expect("toml parses");
        let error = DockerConfig::from_section(&section).expect_err("setup command rejected");
        let message = error.to_string();
        assert!(message.contains("setup-command"), "{message}");
        assert!(message.contains("[[provision]]"), "{message}");
    }
}
