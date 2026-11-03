//! The environment a command runs with, and where it came from.
//!
//! Values live here because the backend must hand them to the process. The
//! provenance deliberately does not: a surface that reports an environment
//! reports names, origins and kinds of change, never what a variable holds.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Where the environment as a whole came from.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Origin {
    /// Captured on the host from a development environment.
    Captured {
        /// The description of what was captured.
        from: String,
    },
    /// Provided by the sandbox image or profile.
    Backend {
        /// The backend's own description.
        from: String,
    },
    /// Nothing was supplied; the sandbox's default applies.
    Default,
}

/// What the backend did to one variable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Change {
    /// Passed through unchanged.
    Unchanged,
    /// Set to a value the backend chose.
    Set,
    /// Repointed at a path inside the sandbox.
    Repointed,
    /// Removed before the command ran.
    Removed,
}

/// One variable's provenance, without its value.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Variable {
    /// The variable's name.
    pub name: String,
    /// What the backend did to it.
    pub change: Change,
}

/// A host path an environment value names, reported rather than granted.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HostPath {
    /// The variable that named it.
    pub variable: String,
    /// The path it named.
    pub path: PathBuf,
}

/// Where the environment came from and what was changed about it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Provenance {
    /// The source of the environment as a whole.
    pub origin: Origin,
    /// One entry per variable the backend knows about.
    pub variables: Vec<Variable>,
    /// Host paths that were reported and not granted.
    pub host_paths: Vec<HostPath>,
}

/// The environment a command receives, with its provenance.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Environment {
    /// Variable names and values, in the order the backend will apply them.
    pub variables: BTreeMap<String, String>,
    /// Where they came from and what was changed.
    pub provenance: Provenance,
    /// The path the sandbox sees the workspace at.
    pub workspace: String,
}

impl Environment {
    /// An environment the backend did not alter.
    pub fn unchanged(
        origin: Origin,
        variables: BTreeMap<String, String>,
        workspace: impl Into<String>,
    ) -> Self {
        let provenance = Provenance {
            origin,
            variables: variables
                .keys()
                .map(|name| Variable {
                    name: name.clone(),
                    change: Change::Unchanged,
                })
                .collect(),
            host_paths: Vec::new(),
        };
        Self {
            variables,
            provenance,
            workspace: workspace.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{Change, Environment, HostPath, Origin};

    fn environment() -> Environment {
        let variables = BTreeMap::from([
            (
                "SSH_AUTH_SOCK".to_string(),
                "/run/user/1000/ssh-agent.sock".to_string(),
            ),
            ("PATH".to_string(), "/usr/bin".to_string()),
        ]);
        Environment::unchanged(
            Origin::Captured {
                from: "nix print-dev-env".to_string(),
            },
            variables,
            "/sandbox",
        )
    }

    #[test]
    fn provenance_reports_names_and_kinds_but_not_values() {
        let environment = environment();
        let names: Vec<&str> = environment
            .provenance
            .variables
            .iter()
            .map(|variable| variable.name.as_str())
            .collect();
        assert!(names.contains(&"SSH_AUTH_SOCK"));
        let rendered = format!("{:?}", environment.provenance);
        assert!(
            !rendered.contains("/run/user/1000"),
            "provenance must not carry a value: {rendered}"
        );
    }

    #[test]
    fn a_host_path_is_reported_not_granted() {
        let mut environment = environment();
        environment.provenance.host_paths.push(HostPath {
            variable: "SSH_AUTH_SOCK".to_string(),
            path: "/run/user/1000/ssh-agent.sock".into(),
        });
        assert_eq!(environment.provenance.host_paths.len(), 1);
        assert!(environment.variables.contains_key("SSH_AUTH_SOCK"));
    }

    #[test]
    fn an_unchanged_environment_records_no_changes() {
        let environment = environment();
        assert!(
            environment
                .provenance
                .variables
                .iter()
                .all(|variable| variable.change == Change::Unchanged)
        );
    }
}
