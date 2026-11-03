//! Docker's construction point.
//!
//! The registry hands over the mechanism's own section; everything
//! Docker-specific happens on this side of the boundary.

use std::sync::Arc;

use super::backend::{BACKEND, DockerBackend};
use super::compute::DockerCompute;
use super::config::DockerConfig;
use crate::sandbox::contract::{Backend, Error};
use crate::sandbox::facets::{Forwarding, Lifecycle, Limits};
use crate::sandbox::selected::Selected;

/// Build Docker's backend from the user's Docker section.
pub fn build(section: &toml::Value) -> Result<Selected, Error> {
    let settings = DockerConfig::from_section(section)?.settings()?;
    let compute = DockerCompute::connect()?;
    let backend = Arc::new(DockerBackend::new(compute, settings));

    let core: Arc<dyn Backend> = backend.clone();
    let lifecycle: Arc<dyn Lifecycle> = backend.clone();
    let limits: Arc<dyn Limits> = backend.clone();
    let forwarding: Arc<dyn Forwarding> = backend;

    Ok(Selected::new(BACKEND, core)
        .with_lifecycle(lifecycle)
        .with_limits(limits)
        .with_forwarding(forwarding))
}
