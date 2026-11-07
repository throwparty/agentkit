//! The compiled-in table of provisioning kinds.
//!
//! Adding a kind is one module and one entry here. There is no discovery and
//! no dynamic loading.

use super::{Kind, command::CommandKind, devshell::DevshellKind};

static COMMAND: CommandKind = CommandKind;
static DEVSHELL: DevshellKind = DevshellKind;

#[cfg(test)]
static TEST_ONLY: TestOnlyKind = TestOnlyKind;

/// Every kind this build contains.
#[allow(unused_mut)]
pub fn all() -> Vec<&'static dyn Kind> {
    let mut kinds: Vec<&'static dyn Kind> = vec![&COMMAND, &DEVSHELL];
    #[cfg(test)]
    kinds.push(&TEST_ONLY);
    kinds
}

/// Look a kind up by the name configuration uses.
pub fn lookup(name: &str) -> Option<&'static dyn Kind> {
    all().into_iter().find(|kind| kind.describe().name == name)
}

/// A test-only kind that has no contract variant.
#[cfg(test)]
#[derive(Debug)]
struct TestOnlyKind;

#[cfg(test)]
impl Kind for TestOnlyKind {
    fn describe(&self) -> super::KindDescription {
        super::KindDescription {
            name: "test-only",
            placement: crate::sandbox::contract::Placement::Host,
            sorts: &[crate::sandbox::contract::ContributionSort::Environment],
            requires_consent: false,
            max_per_sequence: None,
        }
    }

    fn validate(&self, settings: &super::Settings) -> Result<(), super::SettingsError> {
        super::reject_unknown(settings, &["value"])
    }
}
