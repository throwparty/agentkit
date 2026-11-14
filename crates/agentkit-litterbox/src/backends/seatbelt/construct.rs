//! Construct the backend from its section.

use std::sync::Arc;

use super::BACKEND;
use super::backend::SeatbeltBackend;
use super::config::SeatbeltConfig;
use super::host::{SharedHost, SystemHost};
use crate::sandbox::contract::Error;
use crate::sandbox::selected::Selected;

/// Build the backend from its configuration section.
pub(crate) fn build(section: &toml::Value) -> Result<Selected, Error> {
    build_with(section, Arc::new(SystemHost::new()))
}

/// Build the backend over an explicit host, for tests.
pub(crate) fn build_with(section: &toml::Value, host: SharedHost) -> Result<Selected, Error> {
    let settings = SeatbeltConfig::from_section(section)?.settings()?;
    let backend = Arc::new(SeatbeltBackend::new(settings, host)?);
    Ok(Selected::new(BACKEND, backend.clone()).with_lifecycle(backend))
}

#[cfg(test)]
mod tests {
    use super::build;
    use crate::sandbox::contract::Error;

    #[test]
    fn the_empty_section_builds_the_backend() {
        let section = toml::Value::Table(Default::default());
        let selected = build(&section).expect("built");
        assert_eq!(selected.name(), "seatbelt");
    }

    #[test]
    fn a_bad_section_is_misconfigured() {
        let section: toml::Value = toml::from_str("toolchain = 'auto'").expect("toml");
        assert!(matches!(build(&section), Err(Error::Misconfigured { .. })));
    }
}
