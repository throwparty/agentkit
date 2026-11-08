//! Construct the backend from its section.
//!
//! The table in `backends/registry.rs` points here, so this is the only place
//! that turns the machine-owned section into a backend.

use std::sync::Arc;

use super::BACKEND;
use super::backend::BubblewrapBackend;
use super::config::BubblewrapConfig;
use super::host::{SharedHost, SystemHost};
use crate::sandbox::contract::Error;
use crate::sandbox::selected::Selected;

/// Build the backend from its configuration section.
pub fn build(section: &toml::Value) -> Result<Selected, Error> {
    build_with(section, Arc::new(SystemHost::new()))
}

/// Build the backend over an explicit host, for tests.
pub fn build_with(section: &toml::Value, host: SharedHost) -> Result<Selected, Error> {
    let settings = BubblewrapConfig::from_section(section)?.settings()?;
    Ok(Selected::new(
        BACKEND,
        Arc::new(BubblewrapBackend::new(settings, host)),
    ))
}

#[cfg(test)]
mod tests {
    use super::build;
    use crate::sandbox::contract::Error;

    #[test]
    fn the_section_builds_the_backend() {
        let section: toml::Value = toml::from_str("memory-mb = 2048\ntasks = 256").expect("toml");
        let selected = build(&section).expect("built");
        assert_eq!(selected.name(), "bubblewrap");
    }

    #[test]
    fn a_bad_section_is_misconfigured() {
        let section: toml::Value = toml::from_str("memory-mb = 0").expect("toml");
        match build(&section) {
            Err(Error::Misconfigured { message }) => {
                assert!(message.contains("memory-mb"), "{message}");
            }
            Err(other) => panic!("expected a configuration error, got {other:?}"),
            Ok(_) => panic!("a zero bound was accepted"),
        }
    }
}
