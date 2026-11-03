//! The sandboxing mechanisms this binary can run, and the one place a backend
//! is constructed.
//!
//! The set is compiled in. Adding a mechanism is one entry here and one module
//! beside it; nothing else in the crate learns its name.

use std::sync::Arc;

use crate::config::Config;
use crate::sandbox::contract::{Error, Sandbox};
use crate::sandbox::facets::{Forwarding, Lifecycle, Limits};

pub mod docker;

/// The names of the backends compiled into this binary.
pub const AVAILABLE: &[&str] = &[docker::BACKEND];

/// The backend used when the user has not selected one.
pub const DEFAULT: &str = docker::BACKEND;

/// One backend, viewed through the contract and the facets it supports.
///
/// Every field points at the same object; the facets are `None` when the
/// mechanism does not implement them.
#[derive(Clone)]
pub struct Selected {
    /// The contract every caller uses.
    pub core: Arc<dyn Sandbox>,
    /// Pausing and resuming, when the mechanism has it.
    pub lifecycle: Option<Arc<dyn Lifecycle>>,
    /// Resource limits, when the mechanism has them.
    pub limits: Option<Arc<dyn Limits>>,
    /// Port forwarding, when the mechanism has it.
    pub forwarding: Option<Arc<dyn Forwarding>>,
}

/// The backend the user selected, or the default.
pub fn selected_name(config: &Config) -> &str {
    config.backend.as_deref().unwrap_or(DEFAULT)
}

/// Construct the selected backend without checking that it can run.
///
/// Introspection needs to build a backend that is not ready so it can report
/// why; everything else should use [`select`].
pub fn build(config: &Config) -> Result<Selected, Error> {
    match selected_name(config) {
        docker::BACKEND => {
            let section = config
                .backends
                .get(docker::BACKEND)
                .cloned()
                .unwrap_or(toml::Value::Table(Default::default()));
            let settings = docker::config::DockerConfig::from_section(&section)?.settings()?;
            let compute = docker::compute::DockerCompute::connect()?;
            let backend = Arc::new(docker::DockerBackend::new(compute, settings));

            let core: Arc<dyn Sandbox> = backend.clone();
            let lifecycle: Arc<dyn Lifecycle> = backend.clone();
            let forwarding: Arc<dyn Forwarding> = backend;

            Ok(Selected {
                core,
                lifecycle: Some(lifecycle),
                limits: None,
                forwarding: Some(forwarding),
            })
        }
        other => Err(Error::Unsupported {
            operation: "select a backend".to_string(),
            backend: other.to_string(),
            reason: format!("this binary contains {}", AVAILABLE.join(", ")),
            remedy: "choose one of the backends listed by `litterbox backends`".to_string(),
        }),
    }
}

/// Construct the selected backend only if it can run on this host.
pub async fn select(config: &Config) -> Result<Selected, Error> {
    let selected = build(config)?;
    let readiness = selected.core.readiness().await?;
    if readiness.is_available() {
        return Ok(selected);
    }

    let failing = readiness
        .checks
        .iter()
        .find(|check| check.state != crate::sandbox::report::State::Available);

    Err(Error::Unsupported {
        operation: "create a sandbox".to_string(),
        backend: readiness.backend.clone(),
        reason: failing
            .and_then(|check| check.reason.clone())
            .unwrap_or_else(|| "the backend is not ready on this host".to_string()),
        remedy: failing
            .and_then(|check| check.remedy.clone())
            .unwrap_or_else(|| "run `litterbox backends` to see what is missing".to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::{AVAILABLE, DEFAULT, Selected, build, selected_name};
    use crate::config::Config;

    #[test]
    fn docker_is_the_only_backend_and_the_default() {
        assert_eq!(AVAILABLE, &["docker"]);
        assert_eq!(DEFAULT, "docker");
        assert_eq!(selected_name(&Config::default()), "docker");
    }

    #[test]
    fn the_user_can_name_a_backend() {
        let config = Config {
            backend: Some("docker".to_string()),
            ..Config::default()
        };
        assert_eq!(selected_name(&config), "docker");
    }

    #[tokio::test]
    async fn an_unknown_backend_lists_what_exists() {
        let config = Config {
            backend: Some("bubblewrap".to_string()),
            ..Config::default()
        };
        let error = match build(&config) {
            Ok(_) => panic!("unknown backend accepted"),
            Err(error) => error,
        };
        let message = error.to_string();
        assert!(message.contains("bubblewrap"), "{message}");
        assert!(message.contains("docker"), "{message}");
    }

    #[tokio::test]
    async fn docker_exposes_its_facets() {
        let result = build(&Config::default());
        let Ok(selected) = result else {
            // No Docker CLI or daemon in this environment; construction is
            // covered by the daemon-gated backend tests.
            return;
        };
        assert!(selected.lifecycle.is_some());
        assert!(selected.forwarding.is_some());
        assert!(selected.limits.is_none());

        let Selected { core, .. } = selected;
        let readiness = core.readiness().await.expect("readiness");
        assert_eq!(readiness.backend, "docker");
    }
}
