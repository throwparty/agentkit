//! The sandboxing mechanisms this binary can run, and the one place a backend
//! is selected or constructed.
//!
//! One table names each mechanism and how to build it. Adding a backend is one
//! entry and one module.

use serde::Serialize;

use crate::config::{Config, escaped};
use crate::sandbox::contract::{ContributionSupport, Error};
use crate::sandbox::report::{Capability, Readiness, State};
use crate::sandbox::selected::Selected;

use crate::backends::docker;

/// A backend builds itself from its own section.
type Constructor = fn(&toml::Value) -> Result<Selected, Error>;

/// Every mechanism compiled into this binary, with its constructor.
const BACKENDS: &[(&str, Constructor)] = &[(docker::BACKEND, docker::construct::build)];

/// The backend used when the user has not selected one.
pub const DEFAULT: &str = docker::BACKEND;

/// The names of the backends compiled into this binary.
pub fn available_names() -> Vec<&'static str> {
    BACKENDS.iter().map(|(name, _)| *name).collect()
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
    let name = selected_name(config);
    match BACKENDS.iter().find(|(candidate, _)| *candidate == name) {
        Some((_, constructor)) => {
            let section = config
                .backend_settings
                .get(name)
                .cloned()
                .unwrap_or_else(|| toml::Value::Table(Default::default()));
            constructor(&section)
        }
        None => {
            let display = escaped(name);
            let available = available_names()
                .iter()
                .map(|candidate| escaped(candidate))
                .collect::<Vec<_>>()
                .join(", ");
            Err(Error::Unavailable {
                backend: display.clone(),
                reason: format!(
                    "this binary does not build a backend named '{display}'; it contains \
                     {available}"
                ),
                remedy: "choose one of the backends listed by `litterbox backends`".to_string(),
            })
        }
    }
}

/// Construct the selected backend only if it can run on this host.
pub async fn select(config: &Config) -> Result<Selected, Error> {
    select_as(config, "create a sandbox").await
}

/// Why a backend may not serve an operation, in the terms to report.
struct Refusal {
    reason: String,
    remedy: String,
}

/// Decide whether readiness permits the operation.
fn refusal(readiness: &Readiness, accept_degraded: bool) -> Option<Refusal> {
    let unusable = readiness.state() == State::Unavailable;
    // A degraded check with a fallback means the mechanism substitutes
    // something weaker. That is refused unless the user accepted it.
    let bounded_degradation = !accept_degraded
        && readiness
            .checks
            .iter()
            .any(|check| check.state.needs_acceptance(&check.fallback));

    if !unusable && !bounded_degradation {
        return None;
    }

    let failing: Vec<&crate::sandbox::report::Check> = readiness
        .checks
        .iter()
        .filter(|check| check.state != State::Available)
        .collect();

    let remedies: Vec<String> = failing
        .iter()
        .filter_map(|check| check.remedy.clone())
        .collect();
    let base = if remedies.is_empty() {
        "run `litterbox backends` to see what is missing".to_string()
    } else {
        remedies.join("; ")
    };
    let remedy = if bounded_degradation {
        format!("{base}; set accept-degraded = true in the user configuration to proceed anyway")
    } else {
        base
    };

    Some(Refusal {
        reason: failing
            .iter()
            .map(|check| {
                format!(
                    "{} is {:?}: {}",
                    check.primitive,
                    check.state,
                    check
                        .reason
                        .clone()
                        .unwrap_or_else(|| "not available on this host".to_string())
                )
            })
            .collect::<Vec<_>>()
            .join("; "),
        remedy,
    })
}

/// Name the attempted operation in an unavailable report.
fn with_operation(operation: &str, error: Error) -> Error {
    match error {
        Error::Unavailable {
            backend,
            reason,
            remedy,
        } => Error::Unavailable {
            backend,
            reason: format!("{operation}: {reason}"),
            remedy,
        },
        other => other,
    }
}

/// Whether readiness permits an operation under the same policy `select` uses.
pub fn readiness_is_usable(readiness: &Readiness, accept_degraded: bool) -> bool {
    refusal(readiness, accept_degraded).is_none()
}

/// Construct the selected backend for one operation, if it can run here.
pub async fn select_as(config: &Config, operation: &str) -> Result<Selected, Error> {
    let selected = build(config)?;
    let readiness = selected.core().readiness().await?;

    if let Some(refusal) = refusal(&readiness, config.accept_degraded) {
        return Err(with_operation(
            operation,
            Error::Unavailable {
                backend: readiness.backend.clone(),
                reason: refusal.reason,
                remedy: refusal.remedy,
            },
        ));
    }

    Ok(selected)
}

/// One backend's readiness, as introspection reports it.
#[derive(Clone, Debug, Serialize)]
pub struct BackendReport {
    /// The backend's name.
    pub name: String,
    /// Whether the user selected it.
    pub selected: bool,
    /// Its readiness, when it could be built.
    pub readiness: Option<Readiness>,
    /// The optional operations it provides.
    pub capabilities: Vec<Capability>,
    /// Why it could not be built at all.
    pub error: Option<String>,
}

/// Report every compiled-in backend, its readiness and its capability entries.
///
/// This builds a backend even when it cannot run, so the report can say why.
/// One backend's readiness, capabilities and provisioning support.
pub async fn describe(
    backend: &Selected,
) -> Result<(Readiness, Vec<Capability>, Vec<ContributionSupport>), Error> {
    let support = backend.core().contribution_support();
    let (readiness, capabilities) =
        tokio::join!(backend.core().readiness(), backend.core().capabilities());
    let readiness = readiness?;
    let capabilities = backend.merge_capabilities(capabilities?, &support);
    Ok((readiness, capabilities, support))
}

pub async fn introspect(config: &Config) -> Vec<BackendReport> {
    let selected = selected_name(config);
    let mut reports = Vec::new();

    for (name, _) in BACKENDS {
        let mut candidate = config.clone();
        candidate.backend = Some((*name).to_string());
        let is_selected = *name == selected;

        let report = match build(&candidate) {
            Ok(backend) => match describe(&backend).await {
                Ok((readiness, capabilities, _)) => BackendReport {
                    name: (*name).to_string(),
                    selected: is_selected,
                    readiness: Some(readiness),
                    capabilities,
                    error: None,
                },
                Err(error) => BackendReport {
                    name: (*name).to_string(),
                    selected: is_selected,
                    readiness: None,
                    capabilities: Vec::new(),
                    error: Some(error.to_string()),
                },
            },
            Err(error) => BackendReport {
                name: (*name).to_string(),
                selected: is_selected,
                readiness: None,
                capabilities: Vec::new(),
                error: Some(error.to_string()),
            },
        };

        reports.push(report);
    }

    reports
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT, available_names, build, refusal, selected_name};
    use crate::config::Config;
    use crate::sandbox::contract::Error;
    use crate::sandbox::report::{Check, Fallback, Readiness, State};

    #[test]
    fn an_unbuilt_backend_name_is_escaped_in_the_report() {
        let config = Config {
            backend: Some("bad\nname".to_string()),
            ..Config::default()
        };
        match build(&config) {
            Err(Error::Unavailable {
                backend, reason, ..
            }) => {
                assert_eq!(backend, "bad\\nname");
                assert!(!reason.contains('\n'), "{reason}");
            }
            Err(other) => panic!("expected Unavailable, got {other:?}"),
            Ok(_) => panic!("expected the unbuilt backend to be refused"),
        }
    }

    #[test]
    fn an_unbuilt_backend_is_unavailable_not_unsupported() {
        let config = Config {
            backend: Some("bubblewrap".to_string()),
            ..Config::default()
        };
        match build(&config) {
            Err(Error::Unavailable {
                backend, reason, ..
            }) => {
                assert_eq!(backend, "bubblewrap");
                assert!(reason.contains("docker"), "{reason}");
            }
            Err(other) => panic!("expected Unavailable, got {other:?}"),
            Ok(_) => panic!("expected the unbuilt backend to be refused"),
        }
    }

    fn check(primitive: &str, state: State, fallback: Option<Fallback>) -> Check {
        Check {
            primitive: primitive.to_string(),
            state,
            reason: Some(format!("{primitive} is {state:?}")),
            remedy: Some(format!("fix {primitive}")),
            fallback,
        }
    }

    fn readiness(checks: Vec<Check>) -> Readiness {
        Readiness {
            backend: "test".to_string(),
            checks,
        }
    }

    #[test]
    fn a_refusal_names_the_operation_that_was_attempted() {
        let error = super::with_operation(
            "pause",
            crate::sandbox::contract::Error::Unavailable {
                backend: "test".to_string(),
                reason: "a daemon is Unavailable".to_string(),
                remedy: "start it".to_string(),
            },
        );
        let message = error.to_string();
        assert!(message.contains("pause"), "{message}");
        assert!(message.contains("a daemon"), "{message}");
    }

    #[test]
    fn an_available_backend_is_not_refused() {
        let readiness = readiness(vec![check("a daemon", State::Available, None)]);
        assert!(refusal(&readiness, false).is_none());
    }

    #[test]
    fn an_unavailable_backend_names_every_failing_primitive() {
        let readiness = readiness(vec![
            check("a daemon", State::Unavailable, None),
            check("a namespace", State::Unavailable, None),
            check("an image", State::Available, None),
        ]);
        let stopped = refusal(&readiness, true).expect("refused");
        assert!(stopped.reason.contains("a daemon"), "{}", stopped.reason);
        assert!(stopped.reason.contains("a namespace"), "{}", stopped.reason);
        assert!(!stopped.reason.contains("an image"), "{}", stopped.reason);
        assert!(
            stopped.remedy.contains("fix a daemon"),
            "{}",
            stopped.remedy
        );
        assert!(
            !stopped.remedy.contains("accept-degraded"),
            "an unavailable backend cannot be accepted: {}",
            stopped.remedy
        );
    }

    #[test]
    fn a_fallback_is_refused_until_the_user_accepts_degradation() {
        let readiness = readiness(vec![check(
            "a namespace",
            State::Degraded,
            Some(Fallback {
                name: "host networking".to_string(),
                does_not_bound: "egress".to_string(),
            }),
        )]);

        let bounded = refusal(&readiness, false).expect("refused");
        assert!(
            bounded.remedy.contains("accept-degraded = true"),
            "{}",
            bounded.remedy
        );
        assert!(refusal(&readiness, true).is_none());
    }

    #[test]
    fn a_degraded_check_without_a_fallback_is_not_refused() {
        let readiness = readiness(vec![check("an image", State::Degraded, None)]);
        assert!(refusal(&readiness, false).is_none());
    }

    #[test]
    fn docker_is_compiled_in_and_is_the_default() {
        assert!(available_names().contains(&"docker"));
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

    #[test]
    fn an_unknown_backend_lists_what_exists() {
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

    #[test]
    fn an_absent_limits_facet_is_reported_structurally() {
        let backend = std::sync::Arc::new(crate::testing::TestBackend::new());
        let selected = crate::sandbox::selected::Selected::new("test", backend);

        match selected.limits_or_unsupported("set limits") {
            Err(crate::sandbox::contract::Error::Unsupported {
                operation,
                backend,
                primitive,
                reason,
                remedy,
            }) => {
                assert_eq!(operation, "set limits");
                assert_eq!(backend, "test");
                assert!(!primitive.is_empty());
                assert!(!reason.is_empty());
                assert!(!remedy.is_empty());
            }
            Err(other) => panic!("expected an unsupported report, got {other}"),
            Ok(_) => panic!("an absent limits facet was accepted"),
        }
    }

    #[test]
    fn an_absent_facet_reports_its_operation_primitive_and_remedy() {
        let backend = std::sync::Arc::new(crate::testing::TestBackend::new());
        let selected = crate::sandbox::selected::Selected::new("test", backend);

        let error = match selected.lifecycle_or_unsupported("pause") {
            Ok(_) => panic!("a backend without the facet must refuse"),
            Err(error) => error,
        };
        match error {
            crate::sandbox::contract::Error::Unsupported {
                operation,
                backend,
                primitive,
                reason,
                remedy,
            } => {
                assert_eq!(operation, "pause");
                assert_eq!(backend, "test");
                assert!(!primitive.is_empty());
                assert!(!reason.is_empty());
                assert!(!remedy.is_empty());
            }
            other => panic!("expected an unsupported report, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn introspection_derives_standard_capabilities_from_the_facets() {
        let reports = super::introspect(&Config::default()).await;
        let docker = reports
            .iter()
            .find(|report| report.name == "docker")
            .expect("docker is reported");
        let operations: Vec<&str> = docker
            .capabilities
            .iter()
            .map(|capability| capability.operation.as_str())
            .collect();
        assert!(operations.contains(&"pause and resume"), "{operations:?}");
        assert!(operations.contains(&"resource limits"), "{operations:?}");
        assert!(operations.contains(&"port forwarding"), "{operations:?}");
        assert!(operations.contains(&"proxied egress"), "{operations:?}");
        assert!(
            docker
                .capabilities
                .iter()
                .all(|capability| !capability.operation.starts_with("contribution:")),
            "Docker reports no contribution support"
        );
    }

    #[tokio::test]
    async fn introspection_marks_exactly_the_selected_backend() {
        let reports = super::introspect(&Config::default()).await;
        let selected: Vec<&str> = reports
            .iter()
            .filter(|report| report.selected)
            .map(|report| report.name.as_str())
            .collect();
        assert_eq!(selected, vec![super::selected_name(&Config::default())]);

        let docker = reports
            .iter()
            .find(|report| report.name == "docker")
            .expect("docker is reported");
        assert!(
            docker.readiness.is_some() || docker.error.is_some(),
            "a backend report says how it ran or why it could not"
        );
    }
}
