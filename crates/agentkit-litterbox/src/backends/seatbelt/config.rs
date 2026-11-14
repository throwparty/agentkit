//! The Seatbelt backend's machine-owned configuration section.
//!
//! Four keys: `network`, `baseline`, `baseline-reads` and `mach-services`. The
//! last two are additions. `baseline = 'none'` replaces the default read and
//! service lists.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::{defaults, path};
use crate::sandbox::contract::Error;

/// How the sandbox reaches the network.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum NetworkSetting {
    /// No network-outbound rule at all.
    #[default]
    Denied,
    /// Every host and port.
    Host,
}

/// Whether the default read and service lists apply.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum BaselineSetting {
    /// Defaults plus additions.
    #[default]
    Default,
    /// The lists are exactly what the machine wrote.
    None,
}

/// The section as written.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SeatbeltConfig {
    pub(crate) network: Option<NetworkSetting>,
    pub(crate) baseline: Option<BaselineSetting>,
    #[serde(rename = "baseline-reads", default)]
    pub(crate) baseline_reads: Vec<String>,
    #[serde(rename = "mach-services", default)]
    pub(crate) mach_services: Vec<String>,
}

/// The resolved section the backend uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    pub(crate) network: NetworkSetting,
    pub(crate) baseline: BaselineSetting,
    pub(crate) baseline_reads: Vec<PathBuf>,
    pub(crate) mach_services: Vec<String>,
}

impl SeatbeltConfig {
    /// Parse the backend's section from the core's untyped map.
    pub(crate) fn from_section(section: &toml::Value) -> Result<Self, Error> {
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
                message: format!("backends.seatbelt is not valid: {error}"),
            })
    }

    /// Resolve the section against this host's real home.
    pub(crate) fn settings(&self) -> Result<Settings, Error> {
        let home = path::real_home()?;
        self.settings_with_home(&home)
    }

    /// Resolve the section against an explicit home, for tests.
    pub(crate) fn settings_with_home(&self, home: &Path) -> Result<Settings, Error> {
        let baseline = self.baseline.unwrap_or_default();
        let network = self.network.unwrap_or_default();

        let mut baseline_reads = match baseline {
            BaselineSetting::Default => {
                defaults::BASELINE_READS.iter().map(PathBuf::from).collect()
            }
            BaselineSetting::None => Vec::new(),
        };
        for value in &self.baseline_reads {
            push_unique(
                &mut baseline_reads,
                path::parse(value, home, "baseline-reads")?,
            );
        }

        let mut mach_services = match baseline {
            BaselineSetting::Default => defaults::MACH_SERVICES
                .iter()
                .map(|service| service.to_string())
                .collect(),
            BaselineSetting::None => Vec::new(),
        };
        for value in &self.mach_services {
            validate_service(value)?;
            if !mach_services.iter().any(|service| service == value) {
                mach_services.push(value.clone());
            }
        }

        Ok(Settings {
            network,
            baseline,
            baseline_reads,
            mach_services,
        })
    }
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

fn validate_service(value: &str) -> Result<(), Error> {
    super::policy::validate_service(value).map_err(|reason| Error::Misconfigured {
        message: format!("backends.seatbelt.mach-services entry '{value}' {reason}"),
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{BaselineSetting, NetworkSetting, SeatbeltConfig};

    const HOME: &str = "/Users/tester";

    fn config(toml_text: &str) -> SeatbeltConfig {
        let section: toml::Value = toml::from_str(toml_text).expect("toml parses");
        SeatbeltConfig::from_section(&section).expect("section parses")
    }

    #[test]
    fn the_defaults_are_used() {
        let settings = config("")
            .settings_with_home(Path::new(HOME))
            .expect("settings");
        assert_eq!(settings.network, NetworkSetting::Denied);
        assert_eq!(settings.baseline, BaselineSetting::Default);
        assert!(settings.baseline_reads.contains(&"/usr/bin".into()));
        assert!(!settings.baseline_reads.contains(&"/nix/store".into()));
        assert_eq!(settings.mach_services.len(), 12);
    }

    #[test]
    fn baseline_none_uses_only_the_written_lists() {
        let settings = config(
            "baseline = 'none'\n\
             baseline-reads = ['/opt/toolchain']\n\
             mach-services = ['com.apple.example']",
        )
        .settings_with_home(Path::new(HOME))
        .expect("settings");
        assert_eq!(settings.baseline, BaselineSetting::None);
        assert_eq!(
            settings.baseline_reads,
            vec![std::path::PathBuf::from("/opt/toolchain")]
        );
        assert_eq!(settings.mach_services, vec!["com.apple.example"]);
    }

    #[test]
    fn credential_denies_are_no_longer_a_key() {
        let section: toml::Value =
            toml::from_str("credential-denies = ['~/.ssh']").expect("toml parses");
        let error = SeatbeltConfig::from_section(&section).expect_err("unknown key");
        assert!(error.to_string().contains("credential-denies"), "{error}");
    }

    #[test]
    fn additions_do_not_duplicate_defaults() {
        let settings = config("baseline-reads = ['/usr/bin']\n")
            .settings_with_home(Path::new(HOME))
            .expect("settings");
        assert_eq!(
            settings
                .baseline_reads
                .iter()
                .filter(|value| value.as_path() == Path::new("/usr/bin"))
                .count(),
            1
        );
    }

    #[test]
    fn the_network_posture_parses() {
        assert_eq!(
            config("network = 'host'")
                .settings_with_home(Path::new(HOME))
                .expect("settings")
                .network,
            NetworkSetting::Host
        );
        let error = toml::from_str::<toml::Value>("network = 'sometimes'")
            .expect("toml")
            .try_into::<SeatbeltConfig>()
            .expect_err("refused");
        assert!(error.to_string().contains("network"), "{error}");
    }

    #[test]
    fn an_unknown_key_is_refused_by_name() {
        let section: toml::Value = toml::from_str("toolchain = 'auto'").expect("toml parses");
        let error = SeatbeltConfig::from_section(&section).expect_err("unknown key");
        assert!(error.to_string().contains("toolchain"), "{error}");
    }

    #[test]
    fn a_setup_command_points_at_the_provision_section() {
        let section: toml::Value =
            toml::from_str("setup-command = 'echo ready'").expect("toml parses");
        let error = SeatbeltConfig::from_section(&section).expect_err("setup command");
        let message = error.to_string();
        assert!(message.contains("setup-command"), "{message}");
        assert!(message.contains("[[provision]]"), "{message}");
    }

    #[test]
    fn a_relative_path_or_bad_service_is_refused() {
        let relative = config("baseline-reads = ['relative/path']")
            .settings_with_home(Path::new(HOME))
            .expect_err("relative path");
        assert!(
            relative.to_string().contains("baseline-reads"),
            "{relative}"
        );

        let service = config("mach-services = ['not a service']")
            .settings_with_home(Path::new(HOME))
            .expect_err("bad service");
        assert!(service.to_string().contains("mach-services"), "{service}");
    }
}
