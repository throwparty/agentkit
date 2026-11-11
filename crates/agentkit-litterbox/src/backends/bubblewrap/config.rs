//! Bubblewrap's configuration section, typed here and nowhere else.
//!
//! The core configuration holds this section as an untyped value; only this
//! module knows which keys bubblewrap accepts, and a key it does not accept is
//! an error rather than something silently ignored.

use serde::Deserialize;

use super::backend::{BubblewrapSettings, Network};
use crate::sandbox::contract::Error;

/// Bubblewrap's keys.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BubblewrapConfig {
    /// Memory ceiling per sandbox, in MiB.
    #[serde(rename = "memory-mb")]
    pub memory_mb: Option<u64>,
    /// Maximum tasks or processes per sandbox.
    pub tasks: Option<u32>,
    /// Whether the sandbox gets its own network namespace.
    pub network: Option<Network>,
    /// The program and leading arguments that run a shell string, overriding
    /// the default of `["sh", "-c"]`.
    pub shell: Option<Vec<String>>,
}

impl BubblewrapConfig {
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
                message: format!("backends.bubblewrap is not valid: {error}"),
            })
    }

    /// The settings a backend needs.
    ///
    /// Both bounds are optional. A bound of zero is refused rather than treated
    /// as unbounded, because zero would silently remove the limit the user
    /// asked for.
    pub fn settings(&self) -> Result<BubblewrapSettings, Error> {
        if let Some(memory_mb) = self.memory_mb
            && memory_mb < 6
        {
            return Err(Error::Misconfigured {
                message: format!(
                    "backends.bubblewrap.memory-mb is {memory_mb}; it must be at least 6 MiB"
                ),
            });
        }
        if self.tasks == Some(0) {
            return Err(Error::Misconfigured {
                message: "backends.bubblewrap.tasks is 0; it must be at least 1".to_string(),
            });
        }
        let shell = match &self.shell {
            None => super::backend::DEFAULT_SHELL.map(str::to_string).to_vec(),
            Some(shell) => {
                if shell.is_empty() || shell.iter().any(String::is_empty) {
                    return Err(Error::Misconfigured {
                        message: "backends.bubblewrap.shell must name a program and its arguments; \
                                  an empty entry is not a command"
                            .to_string(),
                    });
                }
                shell.clone()
            }
        };

        Ok(BubblewrapSettings {
            memory_mb: self.memory_mb,
            tasks: self.tasks,
            network: self.network.unwrap_or_default(),
            shell,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::BubblewrapConfig;

    #[test]
    fn the_network_setting_is_parsed() {
        use super::Network;

        let host =
            BubblewrapConfig::from_section(&toml::from_str("network = 'host'").expect("section"))
                .expect("config");
        assert_eq!(host.settings().expect("settings").network, Network::Host);
        let denied =
            BubblewrapConfig::from_section(&toml::from_str("network = 'denied'").expect("section"))
                .expect("config");
        assert_eq!(
            denied.settings().expect("settings").network,
            Network::Denied
        );
        assert_eq!(
            BubblewrapConfig::default()
                .settings()
                .expect("settings")
                .network,
            Network::Denied
        );
    }

    #[test]
    fn an_unknown_network_is_refused() {
        let error = BubblewrapConfig::from_section(
            &toml::from_str("network = 'sometimes'").expect("section"),
        )
        .expect_err("refused");
        assert!(error.to_string().contains("network"), "{error}");
    }

    #[test]
    fn the_section_carries_its_bounds() {
        let config = BubblewrapConfig::from_section(
            &toml::from_str("memory-mb = 2048\ntasks = 256").expect("section"),
        )
        .expect("config");
        let settings = config.settings().expect("settings");
        assert_eq!(settings.memory_mb, Some(2048));
        assert_eq!(settings.tasks, Some(256));
    }

    #[test]
    fn an_absent_bound_means_no_bound() {
        let settings = BubblewrapConfig::default().settings().expect("settings");
        assert_eq!(settings.memory_mb, None);
        assert_eq!(settings.tasks, None);
    }

    #[test]
    fn the_shell_defaults_to_sh_dash_c() {
        let settings = BubblewrapConfig::default().settings().expect("settings");
        assert_eq!(settings.shell, vec!["sh".to_string(), "-c".to_string()]);
    }

    #[test]
    fn a_configured_shell_replaces_the_default() {
        let section: toml::Value =
            toml::from_str("shell = ['bash', '-lc']").expect("section");
        let config = BubblewrapConfig::from_section(&section).expect("config");
        assert_eq!(
            config.settings().expect("settings").shell,
            vec!["bash".to_string(), "-lc".to_string()]
        );
    }

    #[test]
    fn an_empty_shell_entry_is_refused() {
        let section: toml::Value = toml::from_str("shell = []").expect("section");
        let config = BubblewrapConfig::from_section(&section).expect("config");
        let error = config.settings().expect_err("refused");
        assert!(error.to_string().contains("shell"), "{error}");
    }

    #[test]
    fn a_zero_bound_is_refused() {
        let memory = BubblewrapConfig {
            memory_mb: Some(0),
            ..BubblewrapConfig::default()
        };
        let message = memory.settings().expect_err("zero refused").to_string();
        assert!(message.contains("memory-mb"), "{message}");

        let tasks = BubblewrapConfig {
            tasks: Some(0),
            ..BubblewrapConfig::default()
        };
        let message = tasks.settings().expect_err("zero refused").to_string();
        assert!(message.contains("tasks"), "{message}");
    }

    #[test]
    fn an_unknown_key_is_rejected_by_name() {
        let section: toml::Value =
            toml::from_str("memory-mb = 2048\nhost = 'unix:///tmp/bwrap.sock'").expect("toml");
        let error = BubblewrapConfig::from_section(&section).expect_err("unknown key rejected");
        assert!(error.to_string().contains("host"), "{error}");
    }

    #[test]
    fn a_backend_setup_command_points_at_the_provision_section() {
        let section: toml::Value = toml::from_str("setup-command = 'echo ready'").expect("toml");
        let error = BubblewrapConfig::from_section(&section).expect_err("setup command rejected");
        assert!(error.to_string().contains("setup-command"), "{error}");
        assert!(error.to_string().contains("[[provision]]"), "{error}");
    }
}
