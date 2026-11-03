//! Reporting what this machine can run.
//!
//! Introspection is a user-facing surface, not an agent-facing one: it is how
//! a person picks a mechanism and discovers why one will not start. No MCP
//! tool exposes it.

use serde::Serialize;

use crate::sandbox::report::{Capability, Check, Readiness, State};

/// One backend's readiness, flattened for output.
#[derive(Clone, Debug, Serialize)]
pub struct BackendReport {
    /// The backend's name.
    pub name: String,
    /// Whether the user selected it.
    pub selected: bool,
    /// The worst state across its checks.
    pub state: String,
    /// One entry per primitive.
    pub checks: Vec<CheckReport>,
    /// One entry per optional operation.
    pub capabilities: Vec<CapabilityReport>,
}

/// One primitive's readiness.
#[derive(Clone, Debug, Serialize)]
pub struct CheckReport {
    /// The primitive, in the mechanism's own words.
    pub primitive: String,
    /// Available, degraded or unavailable.
    pub state: String,
    /// Why.
    pub reason: Option<String>,
    /// What the user can do.
    pub remedy: Option<String>,
    /// What a degraded operation does not bound.
    pub does_not_bound: Option<String>,
}

/// One optional operation.
#[derive(Clone, Debug, Serialize)]
pub struct CapabilityReport {
    /// The operation.
    pub operation: String,
    /// Available, degraded or unavailable.
    pub state: String,
    /// Why.
    pub reason: Option<String>,
    /// What the user can do.
    pub remedy: Option<String>,
}

impl BackendReport {
    /// Build a report from what the backend said.
    pub fn new(
        name: &str,
        selected: bool,
        readiness: &Readiness,
        capabilities: &[Capability],
    ) -> Self {
        Self {
            name: name.to_string(),
            selected,
            state: state_label(readiness.state()).to_string(),
            checks: readiness.checks.iter().map(CheckReport::from).collect(),
            capabilities: capabilities.iter().map(CapabilityReport::from).collect(),
        }
    }

    /// Build a report for a backend that could not be constructed at all.
    pub fn unavailable(name: &str, selected: bool, reason: String, remedy: String) -> Self {
        Self {
            name: name.to_string(),
            selected,
            state: state_label(State::Unavailable).to_string(),
            checks: vec![CheckReport {
                primitive: "the backend itself".to_string(),
                state: state_label(State::Unavailable).to_string(),
                reason: Some(reason),
                remedy: Some(remedy),
                does_not_bound: None,
            }],
            capabilities: Vec::new(),
        }
    }

    /// Render the report as the human-readable lines the CLI prints.
    pub fn lines(&self) -> Vec<String> {
        let marker = if self.selected { "*" } else { " " };
        let mut lines = vec![format!("{marker} {} {}", self.name, self.state)];
        for check in &self.checks {
            let mut line = format!("    {}: {}", check.primitive, check.state);
            if let Some(reason) = &check.reason {
                line.push_str(&format!(" ({reason})"));
            }
            if let Some(remedy) = &check.remedy {
                line.push_str(&format!(" -- {remedy}"));
            }
            lines.push(line);
        }
        for capability in &self.capabilities {
            lines.push(format!(
                "    {}: {}",
                capability.operation, capability.state
            ));
        }
        lines
    }
}

impl From<&Check> for CheckReport {
    fn from(check: &Check) -> Self {
        Self {
            primitive: check.primitive.clone(),
            state: state_label(check.state).to_string(),
            reason: check.reason.clone(),
            remedy: check.remedy.clone(),
            does_not_bound: check
                .fallback
                .as_ref()
                .map(|fallback| fallback.does_not_bound.clone()),
        }
    }
}

impl From<&Capability> for CapabilityReport {
    fn from(capability: &Capability) -> Self {
        Self {
            operation: capability.operation.clone(),
            state: state_label(capability.state).to_string(),
            reason: capability.reason.clone(),
            remedy: capability.remedy.clone(),
        }
    }
}

fn state_label(state: State) -> &'static str {
    match state {
        State::Available => "available",
        State::Degraded => "degraded",
        State::Unavailable => "unavailable",
    }
}

#[cfg(test)]
mod tests {
    use super::BackendReport;
    use crate::sandbox::report::{Capability, Check, Fallback, Readiness, State};

    fn readiness() -> Readiness {
        Readiness {
            backend: "docker".to_string(),
            checks: vec![Check {
                primitive: "the Docker daemon".to_string(),
                state: State::Degraded,
                reason: Some("starting".to_string()),
                remedy: Some("wait".to_string()),
                fallback: Some(Fallback {
                    name: "retry".to_string(),
                    does_not_bound: "memory".to_string(),
                }),
            }],
        }
    }

    #[test]
    fn the_report_carries_structured_fields() {
        let report = BackendReport::new(
            "docker",
            true,
            &readiness(),
            &[Capability {
                operation: "pause".to_string(),
                state: State::Available,
                reason: None,
                remedy: None,
            }],
        );
        assert_eq!(report.state, "degraded");
        assert_eq!(report.checks[0].does_not_bound.as_deref(), Some("memory"));
        assert_eq!(report.capabilities[0].operation, "pause");
    }

    #[test]
    fn the_selected_backend_is_marked_in_the_table() {
        let report = BackendReport::new("docker", true, &readiness(), &[]);
        let lines = report.lines();
        assert!(lines[0].starts_with("* docker"), "{lines:?}");
        assert!(lines[1].contains("the Docker daemon"), "{lines:?}");
    }

    #[test]
    fn a_backend_that_cannot_be_built_still_reports_why() {
        let report = BackendReport::unavailable(
            "docker",
            false,
            "backends.docker.image is not set".to_string(),
            "set it".to_string(),
        );
        assert_eq!(report.state, "unavailable");
        let json = serde_json::to_string(&report).expect("serialises");
        assert!(json.contains("backends.docker.image"), "{json}");
    }
}
