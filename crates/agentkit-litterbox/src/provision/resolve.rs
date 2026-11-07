//! Turn a project's provisioning definitions into the ordered steps a backend
//! applies.
//!
//! The resolver is pure: it reads the merged configuration and the selected
//! backend's contribution support, and returns either the steps or a
//! contract-class error. It performs no I/O, so the doctor and create cannot
//! disagree about the sequence.

use std::collections::HashMap;

use crate::config::{Config, ProvisioningIssue, escaped, valid_backend_name};
use crate::sandbox::contract::{
    ContributionSupport, Error, ProvisionStep, ProvisioningClass, ProvisioningFailure,
};
use crate::sandbox::report::State;

use super::kind::{self, KindDescription, SettingsError};

/// A resolved step and what its kind says about it.
#[derive(Clone, Debug)]
pub struct PlannedStep {
    /// The generic step.
    pub step: ProvisionStep,
    /// The kind's placement, contribution sorts and consent requirement.
    pub description: KindDescription,
}

/// One step's outcome in a preflight report.
#[derive(Debug)]
pub enum StepOutcome {
    /// The step resolved and its contributions are supported.
    Resolved(PlannedStep),
    /// The step could not be resolved; the other steps still report.
    Failed(Box<ProvisioningFailure>),
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

/// Turn a kind's settings error into a provisioning failure.
fn kind_issue(step: &ProvisionStep, error: SettingsError) -> Error {
    provisioning(
        ProvisioningClass::Misconfigured,
        None,
        Some(step.identity.as_str()),
        Some(step.kind.as_str()),
        error.reason,
        error.remedy,
    )
}

/// Validate the definitions and sequences, independent of a backend.
fn validate(config: &Config) -> Result<(), Error> {
    config.check_provisioning().map_err(issue)?;
    for step in &config.provision_definitions {
        kind::validate(step).map_err(|error| kind_issue(step, error))?;
    }
    Ok(())
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

/// Resolve each identity independently, keeping a failure beside its step.
///
/// Configuration and compatibility failures still fail the whole call; a
/// per-step support failure is returned in the vector so a preflight report can
/// show every other step.
pub fn preflight_steps(
    config: &Config,
    selected: &str,
    support: &[ContributionSupport],
) -> Result<Vec<StepOutcome>, Error> {
    validate(config)?;
    check_selected(config, selected)?;
    let definitions = sequence_definitions(config, selected)?;
    kind::validate_sequence(selected, &definitions).map_err(|error| {
        provisioning(
            ProvisioningClass::Misconfigured,
            Some(selected.to_string()),
            None,
            None,
            error.reason,
            error.remedy,
        )
    })?;

    let mut outcomes = Vec::with_capacity(definitions.len());
    for definition in definitions {
        let kind = kind::validate(definition).map_err(|error| kind_issue(definition, error))?;
        let description = kind.describe();
        match check_support(
            definition,
            description,
            selected,
            support,
            config.accept_degraded,
        ) {
            Ok(()) => outcomes.push(StepOutcome::Resolved(PlannedStep {
                step: definition.clone(),
                description,
            })),
            Err(Error::Provisioning { failure }) => {
                outcomes.push(StepOutcome::Failed(failure));
            }
            Err(other) => return Err(other),
        }
    }
    Ok(outcomes)
}

/// Resolve the selected backend's sequence, failing on the first step.
///
/// This is the production entry point create uses; `preflight_steps` keeps the
/// per-step outcomes the doctor needs.
pub fn resolve(
    config: &Config,
    selected: &str,
    support: &[ContributionSupport],
) -> Result<Vec<ProvisionStep>, Error> {
    preflight_steps(config, selected, support)?
        .into_iter()
        .map(|outcome| match outcome {
            StepOutcome::Resolved(resolved) => Ok(resolved.step),
            StepOutcome::Failed(failure) => Err(Error::Provisioning { failure }),
        })
        .collect()
}

/// Refuse a step whose contribution sorts the backend cannot apply, or whose
/// degradation the user has not accepted.
fn check_support(
    step: &ProvisionStep,
    description: KindDescription,
    selected: &str,
    support: &[ContributionSupport],
    accept_degraded: bool,
) -> Result<(), Error> {
    for sort in description.sorts {
        let Some(entry) = support.iter().find(|entry| entry.sort == *sort) else {
            return Err(provisioning(
                ProvisioningClass::Unsupported,
                Some(selected.to_string()),
                Some(step.identity.as_str()),
                Some(step.kind.as_str()),
                format!(
                    "the mechanism does not apply the {} contribution",
                    sort.as_str()
                ),
                "select a backend that supports this contribution",
            ));
        };

        match entry.state {
            State::Available => {}
            State::Degraded if !entry.state.needs_acceptance(&entry.fallback) => {}
            State::Degraded if accept_degraded => {}
            State::Degraded => {
                return Err(provisioning(
                    ProvisioningClass::Unavailable,
                    Some(selected.to_string()),
                    Some(step.identity.as_str()),
                    Some(step.kind.as_str()),
                    format!(
                        "the {} contribution needs {}: {}",
                        sort.as_str(),
                        step.kind.as_str(),
                        entry
                            .reason
                            .clone()
                            .unwrap_or_else(|| "the mechanism is degraded".to_string())
                    ),
                    entry.remedy.clone().unwrap_or_else(|| {
                        "set accept-degraded = true in ~/.config/agentkit/litterbox/config.toml"
                            .to_string()
                    }),
                ));
            }
            State::Unavailable => {
                return Err(provisioning(
                    ProvisioningClass::Unsupported,
                    Some(selected.to_string()),
                    Some(step.identity.as_str()),
                    Some(step.kind.as_str()),
                    entry.reason.clone().unwrap_or_else(|| {
                        format!(
                            "the mechanism cannot apply the {} contribution",
                            sort.as_str()
                        )
                    }),
                    entry.remedy.clone().unwrap_or_else(|| {
                        "use a backend or host where this mechanism can apply the step".to_string()
                    }),
                ));
            }
        }
    }
    Ok(())
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
    use super::*;
    use crate::config::{Config, ProjectConfig};
    use crate::sandbox::contract::{ContributionSort, ProvisionStep};
    use crate::sandbox::report::{Fallback, State};

    fn command(identity: &str) -> ProvisionStep {
        ProvisionStep {
            identity: identity.to_string(),
            kind: "command".to_string(),
            settings: [("run".to_string(), toml::Value::String("true".to_string()))].into(),
        }
    }

    fn devshell(identity: &str, installable: &str) -> ProvisionStep {
        ProvisionStep {
            identity: identity.to_string(),
            kind: "devshell".to_string(),
            settings: [(
                "installable".to_string(),
                toml::Value::String(installable.to_string()),
            )]
            .into(),
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

    fn support(sorts: &[ContributionSort], state: State) -> Vec<ContributionSupport> {
        sorts
            .iter()
            .map(|sort| ContributionSupport {
                sort: *sort,
                state,
                reason: Some("the mechanism is degraded".to_string()),
                remedy: Some("accept it".to_string()),
                fallback: None,
            })
            .collect()
    }

    fn fallback() -> Fallback {
        Fallback {
            name: "a weaker mechanism".to_string(),
            does_not_bound: "some of the host".to_string(),
        }
    }

    #[test]
    fn a_sequence_resolves_in_order() {
        let config = config(
            Some(vec!["bubblewrap".to_string()]),
            vec![command("first"), command("second")],
            vec!["second", "first"],
        );
        let steps = resolve(&config, "bubblewrap", &[]).expect("resolves");
        let identities: Vec<&str> = steps.iter().map(ProvisionStep::identity).collect();
        assert_eq!(identities, vec!["second", "first"]);
    }

    #[test]
    fn a_command_step_needs_no_contribution_support() {
        let config = config(
            Some(vec!["bubblewrap".to_string()]),
            vec![command("fetch")],
            vec!["fetch"],
        );
        assert_eq!(
            resolve(&config, "bubblewrap", &[]).expect("resolves"),
            vec![command("fetch")]
        );
    }

    #[test]
    fn a_devshell_step_needs_every_contribution_sort() {
        let config = config(
            Some(vec!["bubblewrap".to_string()]),
            vec![devshell("dev", "./nix")],
            vec!["dev"],
        );

        let error = resolve(&config, "bubblewrap", &[]).expect_err("no support");
        match &error {
            Error::Provisioning { failure } => {
                assert_eq!(failure.class, ProvisioningClass::Unsupported);
                assert_eq!(failure.kind.as_deref(), Some("devshell"));
                assert_eq!(failure.step.as_deref(), Some("dev"));
                assert!(failure.reason.contains("environment"), "{failure:?}");
            }
            other => panic!("expected Provisioning, got {other}"),
        }

        let error = resolve(
            &config,
            "bubblewrap",
            &support(&[ContributionSort::Environment], State::Available),
        )
        .expect_err("no mount support");
        assert!(error.to_string().contains("mount"), "{error}");

        resolve(
            &config,
            "bubblewrap",
            &support(
                &[ContributionSort::Environment, ContributionSort::Mount],
                State::Available,
            ),
        )
        .expect("both sorts supported");
    }

    #[test]
    fn an_unlisted_backend_is_refused() {
        let config = config(
            Some(vec!["seatbelt".to_string()]),
            vec![command("fetch")],
            vec!["fetch"],
        );
        let error = resolve(&config, "bubblewrap", &[]).expect_err("unlisted backend");
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
        let error = resolve(&config, "bubblewrap", &[]).expect_err("unknown identity");
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
        let error = resolve(&config, "bubblewrap", &[]).expect_err("duplicate");
        assert!(
            matches!(error, Error::Provisioning { ref failure } if failure.class == ProvisioningClass::Misconfigured),
            "{error}"
        );
        assert!(error.to_string().contains("fetch"), "{error}");
    }

    #[test]
    fn an_unknown_kind_is_misconfigured() {
        let mut step = command("fetch");
        step.kind = "nix".to_string();
        let config = config(
            Some(vec!["bubblewrap".to_string()]),
            vec![step],
            vec!["fetch"],
        );
        let error = resolve(&config, "bubblewrap", &[]).expect_err("unknown kind");
        match &error {
            Error::Provisioning { failure } => {
                assert_eq!(failure.class, ProvisioningClass::Misconfigured);
                assert_eq!(failure.kind.as_deref(), Some("nix"));
                assert!(failure.reason.contains("nix"), "{failure:?}");
            }
            other => panic!("expected Provisioning, got {other}"),
        }
    }

    #[test]
    fn a_missing_installable_is_misconfigured() {
        let mut step = devshell("dev", "./nix");
        step.settings.clear();
        let config = config(
            Some(vec!["bubblewrap".to_string()]),
            vec![step],
            vec!["dev"],
        );
        let error = resolve(&config, "bubblewrap", &[]).expect_err("missing installable");
        assert!(error.to_string().contains("installable"), "{error}");
    }

    #[test]
    fn an_unavailable_contribution_is_unsupported() {
        let config = config(
            Some(vec!["bubblewrap".to_string()]),
            vec![devshell("dev", "./nix")],
            vec!["dev"],
        );
        let error = resolve(
            &config,
            "bubblewrap",
            &support(
                &[ContributionSort::Environment, ContributionSort::Mount],
                State::Unavailable,
            ),
        )
        .expect_err("unavailable");
        match error {
            Error::Provisioning { failure } => {
                assert_eq!(failure.class, ProvisioningClass::Unsupported);
                assert_eq!(failure.step.as_deref(), Some("dev"));
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
            vec![devshell("dev", "./nix")],
            vec!["dev"],
        );
        let mut support = support(
            &[ContributionSort::Environment, ContributionSort::Mount],
            State::Degraded,
        );
        for entry in &mut support {
            entry.fallback = Some(fallback());
        }
        let error = resolve(&config, "bubblewrap", &support).expect_err("needs acceptance");
        assert!(
            matches!(error, Error::Provisioning { ref failure } if failure.class == ProvisioningClass::Unavailable),
            "{error}"
        );
        config.accept_degraded = true;
        resolve(&config, "bubblewrap", &support).expect("accepted");
    }

    #[test]
    fn a_degraded_contribution_without_a_fallback_is_usable() {
        let config = config(
            Some(vec!["bubblewrap".to_string()]),
            vec![devshell("dev", "./nix")],
            vec!["dev"],
        );
        let support = support(
            &[ContributionSort::Environment, ContributionSort::Mount],
            State::Degraded,
        );
        resolve(&config, "bubblewrap", &support).expect("usable");
    }

    #[test]
    fn a_degraded_or_unavailable_failure_gets_default_reason_and_remedy() {
        let config = config(
            Some(vec!["bubblewrap".to_string()]),
            vec![devshell("dev", "./nix")],
            vec!["dev"],
        );
        let mut degraded = support(
            &[ContributionSort::Environment, ContributionSort::Mount],
            State::Degraded,
        );
        for entry in &mut degraded {
            entry.reason = None;
            entry.remedy = None;
            entry.fallback = Some(fallback());
        }
        match resolve(&config, "bubblewrap", &degraded).expect_err("degraded refused") {
            Error::Provisioning { failure } => {
                assert!(failure.reason.contains("degraded"), "{failure:?}");
                assert!(
                    failure.remedy.contains("accept-degraded = true"),
                    "{failure:?}"
                );
            }
            other => panic!("expected Provisioning, got {other}"),
        }

        let mut unavailable = support(
            &[ContributionSort::Environment, ContributionSort::Mount],
            State::Unavailable,
        );
        for entry in &mut unavailable {
            entry.reason = None;
            entry.remedy = None;
        }
        match resolve(&config, "bubblewrap", &unavailable).expect_err("unavailable") {
            Error::Provisioning { failure } => {
                assert!(!failure.reason.is_empty(), "{failure:?}");
                assert!(!failure.remedy.is_empty(), "{failure:?}");
            }
            other => panic!("expected Provisioning, got {other}"),
        }
    }

    #[test]
    fn two_devshell_steps_are_refused() {
        let config = config(
            Some(vec!["bubblewrap".to_string()]),
            vec![devshell("dev", "./nix"), devshell("dev2", "./other")],
            vec!["dev", "dev2"],
        );
        let error = resolve(
            &config,
            "bubblewrap",
            &support(
                &[ContributionSort::Environment, ContributionSort::Mount],
                State::Available,
            ),
        )
        .expect_err("two devshell steps");
        assert!(error.to_string().contains("dev"), "{error}");
    }

    #[test]
    fn resolve_each_reports_each_failure_without_failing_the_call() {
        let config = config(
            Some(vec!["bubblewrap".to_string()]),
            vec![
                devshell("z-first", "./nix"),
                command("a-second"),
                devshell("b-third", "./other"),
            ],
            vec!["z-first", "a-second", "b-third"],
        );
        let outcomes = preflight_steps(&config, "bubblewrap", &[]).expect("preflight");
        assert_eq!(outcomes.len(), 3);
        assert!(matches!(outcomes[0], StepOutcome::Failed(_)));
        assert!(matches!(outcomes[1], StepOutcome::Resolved(_)));
        assert!(matches!(outcomes[2], StepOutcome::Failed(_)));
    }
}
