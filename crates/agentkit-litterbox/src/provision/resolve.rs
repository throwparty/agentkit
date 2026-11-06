//! Turn a project's provisioning definitions into the ordered steps a backend
//! applies.
//!
//! The resolver is pure: it reads the merged configuration and the selected
//! backend's support, and returns either the steps or a contract-class error.
//! It performs no I/O, so the doctor and create cannot disagree about the
//! sequence.

use std::collections::HashMap;

use crate::config::{Config, ProvisioningIssue, escaped, valid_backend_name};
use crate::sandbox::contract::{
    Error, ProvisionStep, ProvisionSupport, ProvisioningClass, ProvisioningFailure,
};
use crate::sandbox::report::State;

/// A resolved step together with the backend's support entry.
#[derive(Clone, Debug)]
pub struct ResolvedStep {
    /// The typed step.
    pub step: ProvisionStep,
    /// The backend's support entry for its kind.
    pub support: ProvisionSupport,
}

/// One step's outcome in a preflight report.
#[derive(Debug)]
pub enum StepOutcome {
    /// The step resolved and its kind is usable.
    Resolved(ResolvedStep),
    /// The step could not be resolved; the other steps still report.
    Failed(Error),
}

/// Build a structured provisioning failure.
fn provisioning(
    class: ProvisioningClass,
    backend: Option<String>,
    step: Option<&str>,
    kind: Option<&str>,
    reason: String,
    remedy: impl Into<String>,
) -> Error {
    Error::Provisioning {
        failure: Box::new(ProvisioningFailure::new(
            class,
            backend,
            step.map(str::to_string),
            kind.map(str::to_string),
            reason,
            remedy,
        )),
    }
}

/// Turn a configuration issue into a provisioning failure.
fn issue(issue: ProvisioningIssue) -> Error {
    provisioning(
        ProvisioningClass::Misconfigured,
        issue.backend,
        issue.step.as_deref(),
        None,
        issue.reason,
        issue.remedy,
    )
}

/// Validate the definitions and sequences, independent of a backend.
///
/// The rules live in configuration; this wrapper gives the resolver the
/// contract's structured error.
pub fn validate(config: &Config) -> Result<(), Error> {
    config.check_provisioning().map_err(issue)
}

/// Check that the selected backend is well named and the project lists it.
fn check_selected(config: &Config, selected: &str) -> Result<(), Error> {
    if !valid_backend_name(selected) {
        let display = escaped(selected);
        return Err(provisioning(
            ProvisioningClass::Misconfigured,
            Some(display.clone()),
            None,
            None,
            format!("'{display}' is not a backend name"),
            "select a backend by the name `litterbox backends` reports",
        ));
    }

    if let Some(backends) = config.project.backends.as_deref()
        && !backends.is_empty()
        && !backends.iter().any(|backend| backend == selected)
    {
        return Err(provisioning(
            ProvisioningClass::Misconfigured,
            Some(selected.to_string()),
            None,
            None,
            format!(
                "the project's provisioning supports {}, not '{selected}'",
                list(backends)
            ),
            "add it to [project] backends or select a supported backend",
        ));
    }
    Ok(())
}

/// The ordered definitions the selected backend's sequence names.
fn sequence_definitions<'a>(
    config: &'a Config,
    selected: &str,
) -> Result<Vec<&'a ProvisionStep>, Error> {
    let Some(sequence) = config.backend_sequences.get(selected) else {
        return Ok(Vec::new());
    };
    let definitions: HashMap<&str, &ProvisionStep> = config
        .provision_definitions
        .iter()
        .map(|definition| (definition.identity(), definition))
        .collect();

    sequence
        .iter()
        .map(|identity| {
            definitions
                .get(identity.as_str())
                .copied()
                .ok_or_else(|| undefined_identity(selected, identity))
        })
        .collect()
}

/// The structured refusal for a sequence identity no definition names.
fn undefined_identity(selected: &str, identity: &str) -> Error {
    let identity = escaped(identity);
    provisioning(
        ProvisioningClass::Misconfigured,
        Some(selected.to_string()),
        Some(&identity),
        None,
        format!(
            "backends.{selected}.provision names '{identity}', which [[provision]] does not \
             define"
        ),
        "define the step in [[provision]] or remove it from the sequence",
    )
}

/// Resolve the selected backend's sequence, keeping each step's support entry.
#[cfg(test)]
pub fn resolve_steps(
    config: &Config,
    selected: &str,
    support: &[ProvisionSupport],
) -> Result<Vec<ResolvedStep>, Error> {
    validate(config)?;
    check_selected(config, selected)?;
    let definitions = sequence_definitions(config, selected)?;
    let mut steps = Vec::with_capacity(definitions.len());

    for definition in definitions {
        let support = check_support(definition, selected, support, config.accept_degraded)?;
        steps.push(ResolvedStep {
            step: step(definition),
            support,
        });
    }

    Ok(steps)
}

/// Resolve each identity independently, keeping a failure beside its step.
///
/// Configuration and compatibility failures still fail the whole call; a
/// per-step kind or degradation failure is returned in the vector so a
/// preflight report can show every other step.
pub fn preflight_steps(
    config: &Config,
    selected: &str,
    support: &[ProvisionSupport],
) -> Result<Vec<StepOutcome>, Error> {
    validate(config)?;
    check_selected(config, selected)?;
    let definitions = sequence_definitions(config, selected)?;

    Ok(definitions
        .into_iter()
        .map(|definition| {
            match check_support(definition, selected, support, config.accept_degraded) {
                Ok(support) => StepOutcome::Resolved(ResolvedStep {
                    step: step(definition),
                    support,
                }),
                Err(error) => StepOutcome::Failed(error),
            }
        })
        .collect())
}

/// Resolve the selected backend's provisioning sequence.
#[cfg(test)]
pub fn resolve(
    config: &Config,
    selected: &str,
    support: &[ProvisionSupport],
) -> Result<Vec<ProvisionStep>, Error> {
    Ok(resolve_steps(config, selected, support)?
        .into_iter()
        .map(|resolved| resolved.step)
        .collect())
}

/// Refuse a step whose kind the backend cannot apply, or whose degradation the
/// user has not accepted.
fn check_support(
    definition: &ProvisionStep,
    selected: &str,
    support: &[ProvisionSupport],
    accept_degraded: bool,
) -> Result<ProvisionSupport, Error> {
    let kind = definition.kind();
    let Some(entry) = support.iter().find(|entry| entry.kind == kind) else {
        return Err(provisioning(
            ProvisioningClass::Unsupported,
            Some(selected.to_string()),
            Some(definition.identity()),
            Some(kind.as_str()),
            format!("the mechanism does not apply {} steps", kind.as_str()),
            "select a backend that supports this step kind",
        ));
    };

    match entry.state {
        State::Available => Ok(entry.clone()),
        // A degraded kind with no fallback is the mechanism's own weaker mode;
        // the contract gates only a degradation that substitutes something.
        State::Degraded if !entry.state.needs_acceptance(&entry.fallback) => Ok(entry.clone()),
        State::Degraded if accept_degraded => Ok(entry.clone()),
        State::Degraded => Err(provisioning(
            ProvisioningClass::Unavailable,
            Some(selected.to_string()),
            Some(definition.identity()),
            Some(kind.as_str()),
            format!(
                "provisioning step '{}' needs {}: {}",
                definition.identity(),
                kind.as_str(),
                entry
                    .reason
                    .clone()
                    .unwrap_or_else(|| "the mechanism is degraded".to_string())
            ),
            entry.remedy.clone().unwrap_or_else(|| {
                "accept the degraded mechanism in .agentkit/litterbox/config.local.toml".to_string()
            }),
        )),
        State::Unavailable => Err(provisioning(
            ProvisioningClass::Unsupported,
            Some(selected.to_string()),
            Some(definition.identity()),
            Some(kind.as_str()),
            entry
                .reason
                .clone()
                .unwrap_or_else(|| format!("the mechanism cannot apply {} steps", kind.as_str())),
            entry.remedy.clone().unwrap_or_else(|| {
                "use a backend or host where this mechanism can apply the step".to_string()
            }),
        )),
    }
}

/// The typed step a definition becomes.
fn step(definition: &ProvisionStep) -> ProvisionStep {
    match definition {
        ProvisionStep::Command { identity, run } => ProvisionStep::Command {
            identity: identity.clone(),
            run: run.clone(),
        },
    }
}

/// A readable list of backend names for a refusal.
fn list(backends: &[String]) -> String {
    if backends.is_empty() {
        return "no backends".to_string();
    }
    backends
        .iter()
        .map(|backend| format!("'{backend}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::{StepOutcome, preflight_steps, resolve, resolve_steps, validate};
    use crate::config::{Config, ProjectConfig};
    use crate::sandbox::contract::{
        Error, ProvisionKind, ProvisionStep, ProvisionSupport, ProvisioningClass,
    };
    use crate::sandbox::report::{Fallback, State};

    fn command(identity: &str) -> ProvisionStep {
        ProvisionStep::Command {
            identity: identity.to_string(),
            run: "true".to_string(),
        }
    }

    fn config(
        backends: Option<Vec<String>>,
        definitions: Vec<ProvisionStep>,
        sequence: Vec<&str>,
    ) -> Config {
        Config {
            project: ProjectConfig {
                backends,
                ..Default::default()
            },
            provision_definitions: definitions,
            backend_sequences: [(
                "bubblewrap".to_string(),
                sequence.into_iter().map(str::to_string).collect(),
            )]
            .into_iter()
            .collect(),
            ..Config::default()
        }
    }

    fn support(state: State) -> Vec<ProvisionSupport> {
        vec![ProvisionSupport {
            kind: ProvisionKind::Command,
            state,
            reason: Some("the mechanism is degraded".to_string()),
            remedy: Some("accept it".to_string()),
            fallback: None,
        }]
    }

    fn support_with(state: State, fallback: Option<Fallback>) -> Vec<ProvisionSupport> {
        vec![ProvisionSupport {
            kind: ProvisionKind::Command,
            state,
            reason: Some("the mechanism is degraded".to_string()),
            remedy: Some("accept it".to_string()),
            fallback,
        }]
    }

    #[test]
    fn a_sequence_resolves_in_order() {
        let config = config(
            Some(vec!["bubblewrap".to_string()]),
            vec![command("first"), command("second")],
            vec!["second", "first"],
        );
        let steps = resolve(&config, "bubblewrap", &support(State::Available)).expect("resolves");
        assert_eq!(
            steps,
            vec![
                ProvisionStep::Command {
                    identity: "second".to_string(),
                    run: "true".to_string(),
                },
                ProvisionStep::Command {
                    identity: "first".to_string(),
                    run: "true".to_string(),
                },
            ]
        );
    }

    #[test]
    fn an_unlisted_backend_is_refused() {
        let config = config(
            Some(vec!["seatbelt".to_string()]),
            vec![command("fetch")],
            vec!["fetch"],
        );
        let error = resolve(&config, "bubblewrap", &support(State::Available))
            .expect_err("unlisted backend");
        match &error {
            Error::Provisioning { failure } => {
                assert_eq!(failure.class, ProvisioningClass::Misconfigured);
                assert_eq!(failure.backend.as_deref(), Some("bubblewrap"));
                assert!(failure.reason.contains("bubblewrap"), "{failure:?}");
                assert!(failure.reason.contains("seatbelt"), "{failure:?}");
                assert!(failure.remedy.contains("[project] backends"), "{failure:?}");
            }
            other => panic!("expected Provisioning, got {other}"),
        }
    }

    #[test]
    fn an_unknown_identity_is_refused() {
        let config = config(
            Some(vec!["bubblewrap".to_string()]),
            vec![command("fetch")],
            vec!["other"],
        );
        let error = resolve(&config, "bubblewrap", &support(State::Available))
            .expect_err("unknown identity");
        assert!(
            matches!(error, Error::Provisioning { ref failure } if failure.class == ProvisioningClass::Misconfigured),
            "{error}"
        );
        assert!(error.to_string().contains("other"), "{error}");
    }

    #[test]
    fn a_duplicate_identity_is_refused() {
        let config = config(
            Some(vec!["bubblewrap".to_string()]),
            vec![command("fetch")],
            vec!["fetch", "fetch"],
        );
        let error =
            resolve(&config, "bubblewrap", &support(State::Available)).expect_err("duplicate");
        assert!(
            matches!(error, Error::Provisioning { ref failure } if failure.class == ProvisioningClass::Misconfigured),
            "{error}"
        );
        let message = error.to_string();
        assert!(message.contains("fetch"), "{message}");
        assert!(message.contains("bubblewrap"), "{message}");
    }

    #[test]
    fn an_unsupported_kind_is_refused() {
        let config = config(
            Some(vec!["bubblewrap".to_string()]),
            vec![command("fetch")],
            vec!["fetch"],
        );
        match resolve(&config, "bubblewrap", &[]).expect_err("unsupported") {
            Error::Provisioning { ref failure } => {
                assert_eq!(failure.class, ProvisioningClass::Unsupported);
                assert_eq!(failure.backend.as_deref(), Some("bubblewrap"));
                assert_eq!(failure.kind.as_deref(), Some("command"));
                assert_eq!(failure.step.as_deref(), Some("fetch"));
                assert!(!failure.reason.is_empty());
                assert!(!failure.remedy.is_empty());
            }
            other => panic!("expected Provisioning, got {other}"),
        }
    }

    #[test]
    fn an_unavailable_kind_is_unsupported() {
        let config = config(
            Some(vec!["bubblewrap".to_string()]),
            vec![command("fetch")],
            vec!["fetch"],
        );
        match resolve(&config, "bubblewrap", &support(State::Unavailable)).expect_err("unavailable")
        {
            Error::Provisioning { ref failure } => {
                assert_eq!(failure.class, ProvisioningClass::Unsupported);
                assert_eq!(failure.step.as_deref(), Some("fetch"));
                assert!(!failure.reason.is_empty(), "{failure:?}");
                assert!(!failure.remedy.is_empty());
            }
            other => panic!("expected Provisioning, got {other}"),
        }
    }

    #[test]
    fn a_fallback_makes_degradation_need_acceptance() {
        let mut config = config(
            Some(vec!["bubblewrap".to_string()]),
            vec![command("fetch")],
            vec!["fetch"],
        );
        let error = resolve(
            &config,
            "bubblewrap",
            &support_with(
                State::Degraded,
                Some(Fallback {
                    name: "a weaker mechanism".to_string(),
                    does_not_bound: "some of the host".to_string(),
                }),
            ),
        )
        .expect_err("degraded with a fallback refused");
        assert!(
            matches!(error, Error::Provisioning { ref failure } if failure.class == ProvisioningClass::Unavailable),
            "{error}"
        );
        config.accept_degraded = true;
        let resolved = resolve_steps(
            &config,
            "bubblewrap",
            &support_with(
                State::Degraded,
                Some(Fallback {
                    name: "a weaker mechanism".to_string(),
                    does_not_bound: "some of the host".to_string(),
                }),
            ),
        )
        .expect("accepted");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].support.state, State::Degraded);
    }

    #[test]
    fn a_degraded_kind_with_a_fallback_needs_acceptance() {
        let mut config = config(
            Some(vec!["bubblewrap".to_string()]),
            vec![command("fetch")],
            vec!["fetch"],
        );
        let error = resolve_steps(
            &config,
            "bubblewrap",
            &support_with(
                State::Degraded,
                Some(Fallback {
                    name: "a weaker mechanism".to_string(),
                    does_not_bound: "some of the host".to_string(),
                }),
            ),
        )
        .expect_err("degraded with a fallback refused without acceptance");
        assert!(
            matches!(error, Error::Provisioning { ref failure } if failure.class == ProvisioningClass::Unavailable),
            "{error}"
        );
        config.accept_degraded = true;
        resolve_steps(
            &config,
            "bubblewrap",
            &support_with(
                State::Degraded,
                Some(Fallback {
                    name: "a weaker mechanism".to_string(),
                    does_not_bound: "some of the host".to_string(),
                }),
            ),
        )
        .expect("accepted degraded kind");
    }

    #[test]
    fn a_degraded_or_unavailable_failure_gets_default_reason_and_remedy() {
        let config = config(
            Some(vec!["bubblewrap".to_string()]),
            vec![command("fetch")],
            vec!["fetch"],
        );
        let degraded = vec![ProvisionSupport {
            kind: ProvisionKind::Command,
            state: State::Degraded,
            reason: None,
            remedy: None,
            fallback: Some(Fallback {
                name: "a weaker mechanism".to_string(),
                does_not_bound: "some of the host".to_string(),
            }),
        }];
        match resolve(&config, "bubblewrap", &degraded).expect_err("degraded refused") {
            Error::Provisioning { failure } => {
                assert!(failure.reason.contains("degraded"), "{failure:?}");
                assert!(!failure.remedy.is_empty(), "{failure:?}");
            }
            other => panic!("expected Provisioning, got {other}"),
        }

        let unavailable = vec![ProvisionSupport {
            kind: ProvisionKind::Command,
            state: State::Unavailable,
            reason: None,
            remedy: None,
            fallback: None,
        }];
        match resolve(&config, "bubblewrap", &unavailable).expect_err("unavailable") {
            Error::Provisioning { failure } => {
                assert!(!failure.reason.is_empty(), "{failure:?}");
                assert!(!failure.remedy.is_empty(), "{failure:?}");
            }
            other => panic!("expected Provisioning, got {other}"),
        }
    }

    #[test]
    fn resolve_each_reports_each_failure_without_failing_the_call() {
        let mut config = Config {
            project: ProjectConfig {
                backends: Some(vec!["bubblewrap".to_string()]),
                ..Default::default()
            },
            provision_definitions: vec![
                command("z-first"),
                command("a-second"),
                command("b-third"),
            ],
            ..Config::default()
        };
        config.backend_sequences.insert(
            "bubblewrap".to_string(),
            vec![
                "z-first".to_string(),
                "a-second".to_string(),
                "b-third".to_string(),
            ],
        );

        let results =
            preflight_steps(&config, "bubblewrap", &[]).expect("the call itself succeeds");
        assert_eq!(results.len(), 3);
        let identities: Vec<&str> = results
            .iter()
            .map(|outcome| match outcome {
                StepOutcome::Failed(Error::Provisioning { failure }) => {
                    failure.step.as_deref().unwrap_or("")
                }
                _ => "",
            })
            .collect();
        assert_eq!(identities, vec!["z-first", "a-second", "b-third"]);
        assert!(
            results
                .iter()
                .all(|outcome| matches!(outcome, StepOutcome::Failed(_)))
        );
    }

    #[test]
    fn preflight_steps_keeps_a_validation_failure_for_the_whole_call() {
        let config = Config {
            provision_definitions: vec![command("fetch")],
            ..Config::default()
        };
        let error = preflight_steps(&config, "bubblewrap", &[]).expect_err("missing supported set");
        assert!(
            matches!(error, Error::Provisioning { ref failure } if failure.class == ProvisioningClass::Misconfigured),
            "{error}"
        );
    }

    #[test]
    fn a_backend_with_no_sequence_runs_no_steps() {
        let config = Config {
            project: ProjectConfig {
                backends: Some(vec!["bubblewrap".to_string()]),
                ..Default::default()
            },
            provision_definitions: vec![command("fetch")],
            ..Config::default()
        };
        assert!(
            resolve(&config, "bubblewrap", &support(State::Available))
                .expect("resolves")
                .is_empty()
        );
    }

    #[test]
    fn no_definitions_and_no_supported_set_is_empty() {
        let config = Config::default();
        assert!(
            resolve(&config, "bubblewrap", &support(State::Available))
                .expect("resolves")
                .is_empty()
        );
    }

    #[test]
    fn each_backend_resolves_only_its_own_sequence() {
        let mut config = Config {
            project: ProjectConfig {
                backends: Some(vec!["bubblewrap".to_string(), "docker".to_string()]),
                ..Default::default()
            },
            provision_definitions: vec![command("devshell"), command("fetch")],
            ..Config::default()
        };
        config.backend_sequences.insert(
            "bubblewrap".to_string(),
            vec!["devshell".to_string(), "fetch".to_string()],
        );
        config
            .backend_sequences
            .insert("docker".to_string(), vec!["fetch".to_string()]);

        let bubblewrap =
            resolve(&config, "bubblewrap", &support(State::Available)).expect("bubblewrap");
        let docker = resolve(&config, "docker", &support(State::Available)).expect("docker");

        let bubblewrap: Vec<&str> = bubblewrap.iter().map(ProvisionStep::identity).collect();
        let docker: Vec<&str> = docker.iter().map(ProvisionStep::identity).collect();
        assert_eq!(bubblewrap, vec!["devshell", "fetch"]);
        assert_eq!(docker, vec!["fetch"]);
    }

    #[test]
    fn an_invalid_identity_is_refused_by_validation() {
        for identity in ["", "1fetch", "fetch!", "fetch.ok"] {
            let config = Config {
                provision_definitions: vec![command(identity)],
                ..Config::default()
            };
            let error = validate(&config).expect_err("invalid identity");
            assert!(
                matches!(error, Error::Provisioning { ref failure } if failure.class == ProvisioningClass::Misconfigured),
                "{error}"
            );
        }

        let config = Config {
            project: ProjectConfig {
                backends: Some(vec!["bubblewrap".to_string()]),
                ..Default::default()
            },
            provision_definitions: vec![command("fetch_ok-1")],
            ..Config::default()
        };
        validate(&config).expect("a valid identity");
    }

    #[test]
    fn an_invalid_backend_name_is_refused_before_it_reaches_a_message() {
        let config = Config {
            project: ProjectConfig {
                backends: Some(vec!["bad\nname".to_string()]),
                ..Default::default()
            },
            ..Config::default()
        };
        let error = validate(&config).expect_err("control characters refused");
        match error {
            Error::Provisioning { failure } => {
                assert_eq!(failure.class, ProvisioningClass::Misconfigured);
                assert_eq!(failure.backend.as_deref(), Some("bad\\nname"));
                assert!(!failure.reason.contains('\n'), "{failure:?}");
            }
            other => panic!("expected Provisioning, got {other}"),
        }
    }

    #[test]
    fn a_control_character_selected_backend_is_refused() {
        let config = Config::default();
        let error = resolve(&config, "bad\nname", &support(State::Available))
            .expect_err("control characters refused");
        match error {
            Error::Provisioning { failure } => {
                assert_eq!(failure.class, ProvisioningClass::Misconfigured);
                assert_eq!(failure.backend.as_deref(), Some("bad\\nname"));
                assert!(!failure.reason.contains('\n'), "{failure:?}");
            }
            other => panic!("expected Provisioning, got {other}"),
        }
    }

    #[test]
    fn a_non_empty_supported_set_refuses_an_unlisted_backend_without_steps() {
        let config = Config {
            project: ProjectConfig {
                backends: Some(vec!["other".to_string()]),
                ..Default::default()
            },
            ..Config::default()
        };
        let error = resolve(&config, "bubblewrap", &support(State::Available))
            .expect_err("an unlisted backend is refused even without steps");
        assert!(
            matches!(error, Error::Provisioning { ref failure } if failure.class == ProvisioningClass::Misconfigured),
            "{error}"
        );
    }

    #[test]
    fn an_empty_supported_set_with_no_steps_does_not_restrict() {
        let config = Config {
            project: ProjectConfig {
                backends: Some(Vec::new()),
                ..Default::default()
            },
            ..Config::default()
        };
        assert!(
            resolve(&config, "bubblewrap", &support(State::Available))
                .expect("an empty set with no steps is no restriction")
                .is_empty()
        );
    }

    #[test]
    fn a_project_level_issue_names_no_backend() {
        let config = Config {
            provision_definitions: vec![command("fetch"), command("fetch")],
            ..Config::default()
        };
        match validate(&config).expect_err("duplicate") {
            Error::Provisioning { failure } => {
                assert!(failure.backend.is_none(), "{failure:?}");
                assert_eq!(failure.step.as_deref(), Some("fetch"));
            }
            other => panic!("expected Provisioning, got {other}"),
        }
    }
}
