//! The `devshell` kind: a Nix development shell entered on the host.

use crate::sandbox::contract::{ContributionSort, Placement};

use super::{Kind, KindDescription, Settings, SettingsError, reject_unknown, required_string};

/// A development shell entered on the host.
pub struct DevshellKind;

impl Kind for DevshellKind {
    fn describe(&self) -> KindDescription {
        KindDescription {
            name: "devshell",
            placement: Placement::Host,
            sorts: &[ContributionSort::Environment, ContributionSort::Mount],
            requires_consent: true,
            max_per_sequence: Some(1),
        }
    }

    fn validate(&self, settings: &Settings) -> Result<(), SettingsError> {
        reject_unknown(settings, &["installable"])?;
        required_string(settings, "installable")?;
        Ok(())
    }
}
