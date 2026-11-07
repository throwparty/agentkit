//! The `command` kind: a shell command run inside the sandbox.

use crate::sandbox::contract::Placement;

use super::{Kind, KindDescription, Settings, SettingsError, reject_unknown, required_string};

/// A command run once, confined, in the sandbox.
pub struct CommandKind;

impl Kind for CommandKind {
    fn describe(&self) -> KindDescription {
        KindDescription {
            name: "command",
            placement: Placement::Sandbox,
            sorts: &[],
            requires_consent: true,
            max_per_sequence: None,
        }
    }

    fn validate(&self, settings: &Settings) -> Result<(), SettingsError> {
        reject_unknown(settings, &["run"])?;
        required_string(settings, "run")?;
        Ok(())
    }
}
