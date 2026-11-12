//! The environment a sandbox receives.
//!
//! The steps' contributions are merged in order, a value that names an
//! ungranted host path withholds its variable, and the backend's own variables
//! are added. Values live here and in the sandbox's own environment; nothing is
//! written down.

use std::collections::BTreeMap;
use std::path::Path;

use super::view::{HOME, WORKSPACE};
use crate::sandbox::contract::{Error, ResolvedStep};
use crate::sandbox::environment::{
    Change, Environment, Origin, Provenance, StepProvenance, Variable,
};

/// The temporary directory inside the sandbox.
const TMPDIR: &str = "/tmp";

/// Merge the resolved steps into the environment a sandbox receives.
pub fn merge(steps: &[ResolvedStep]) -> Result<Environment, Error> {
    let granted = granted_paths(steps);
    let mut variables: BTreeMap<String, String> = BTreeMap::new();
    let mut provenance = Provenance {
        origin: Origin::Default,
        variables: Vec::new(),
        host_paths: Vec::new(),
        steps: Vec::new(),
    };

    for step in steps {
        let contributions = &step.contributions;
        if contributions.environment.is_empty() && contributions.mounts.is_empty() {
            continue;
        }
        if let Origin::Captured { .. } = contributions.provenance.origin {
            provenance.origin = contributions.provenance.origin.clone();
        }
        let merged = contributions.environment_for(WORKSPACE);
        for (name, value) in merged.variables {
            variables.insert(name, value);
        }
        provenance.steps.push(StepProvenance {
            identity: step.identity.clone(),
            kind: step.kind.clone(),
            variables: contributions.provenance.variables.clone(),
        });
        for path in &contributions.provenance.host_paths {
            provenance.host_paths.push(path.clone());
        }
    }

    let mut withheld = Vec::new();
    let mut trimmed = Vec::new();
    for (name, value) in &variables {
        match verdict(name, value, &granted) {
            Verdict::Keep => {}
            Verdict::Remove => withheld.push(name.clone()),
            Verdict::Trim(value) => trimmed.push((name.clone(), value)),
        }
    }
    for name in &withheld {
        variables.remove(name);
        provenance.variables.push(Variable {
            name: name.clone(),
            change: Change::Removed,
        });
    }
    for (name, value) in trimmed {
        variables.insert(name.clone(), value);
        provenance.variables.push(Variable {
            name,
            change: Change::Set,
        });
    }

    // `PATH` is not the backend's to invent: a command finds what the project's
    // steps contributed, and nothing when they contributed nothing.
    let owned = [
        ("HOME", HOME.to_string()),
        ("TMPDIR", TMPDIR.to_string()),
        ("PWD", WORKSPACE.to_string()),
    ];
    for (name, value) in owned {
        variables.insert(name.to_string(), value);
        provenance.variables.push(Variable {
            name: name.to_string(),
            change: Change::Set,
        });
    }

    Ok(Environment {
        variables,
        provenance,
        workspace: WORKSPACE.to_string(),
    })
}

/// Every path the sandbox can see, as the contributions describe.
fn granted_paths(steps: &[ResolvedStep]) -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    for step in steps {
        for path in &step.contributions.mounts {
            paths.push(path.clone());
        }
    }
    paths
}

/// What the ungranted-path rule does with one variable's value.
enum Verdict {
    /// Every absolute element names something the sandbox can see.
    Keep,
    /// Some elements do not, and can be dropped without emptying the value.
    Trim(String),
    /// Nothing would remain.
    Remove,
}

/// Whether a variable holds a `:`-separated list of paths.
///
/// Only these are read as lists. Every other value is a string whose meaning
/// is its own, and splitting it would corrupt it: `https://example.com` is the
/// obvious case, where `//example.com` is not a path at all.
fn is_path_list(name: &str) -> bool {
    const LISTS: [&str; 6] = [
        "PATH",
        "MANPATH",
        "INFOPATH",
        "PYTHONPATH",
        "NODE_PATH",
        "CLASSPATH",
    ];
    LISTS.contains(&name) || name.ends_with("_PATH")
}

/// Whether a sandbox resolves one path-like value.
fn granted_element(value: &str, granted: &[std::path::PathBuf]) -> bool {
    let element = Path::new(value);
    element == Path::new(WORKSPACE)
        || element.starts_with(WORKSPACE)
        || granted
            .iter()
            .any(|path| element == path || element.starts_with(path))
}

/// Judge the value of the named variable.
fn verdict(name: &str, value: &str, granted: &[std::path::PathBuf]) -> Verdict {
    if !is_path_list(name) {
        // A value that is one absolute path the sandbox cannot see names
        // something absent, so the variable goes. Anything else is kept as it
        // was written: the backend does not guess what a variable is for.
        let absent = value.starts_with('/') && !granted_element(value, granted);
        return if absent {
            Verdict::Remove
        } else {
            Verdict::Keep
        };
    }

    let elements: Vec<&str> = value.split(':').collect();
    let absolute = elements
        .iter()
        .filter(|element| element.starts_with('/'))
        .count();
    if absolute == 0 {
        return Verdict::Keep;
    }
    let kept: Vec<&str> = elements
        .into_iter()
        .filter(|element| !element.starts_with('/') || granted_element(element, granted))
        .collect();
    if kept.len() == value.split(':').count() {
        return Verdict::Keep;
    }
    if kept.is_empty() {
        return Verdict::Remove;
    }
    Verdict::Trim(kept.join(":"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::merge;
    use crate::backends::bubblewrap::view::WORKSPACE;
    use crate::sandbox::contract::{Contributions, Placement, ResolvedStep};
    use crate::sandbox::environment::{Change, Origin, Provenance, Variable};

    fn step(identity: &str, environment: &[(&str, &str)], mounts: &[&str]) -> ResolvedStep {
        let mut value = Contributions::default();
        value.environment = environment
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        value.mounts = mounts.iter().map(PathBuf::from).collect();
        value.provenance = Provenance {
            origin: Origin::Captured {
                from: "nix print-dev-env".to_string(),
            },
            variables: environment
                .iter()
                .map(|(name, _)| Variable {
                    name: name.to_string(),
                    change: Change::Unchanged,
                })
                .collect(),
            host_paths: Vec::new(),
            steps: Vec::new(),
        };
        ResolvedStep {
            identity: identity.to_string(),
            kind: "devshell".to_string(),
            placement: Placement::Host,
            sandbox: None,
            contributions: value,
        }
    }

    #[test]
    fn a_value_that_is_not_a_path_list_is_kept_verbatim() {
        let steps = vec![step(
            "dev",
            &[
                ("SOME_URL", "https://example.com/thing"),
                ("SOME_SPEC", "registry=https://example.com"),
            ],
            &["/nix/store/abc"],
        )];
        let environment = merge(&steps).expect("environment");
        assert_eq!(
            environment.variables.get("SOME_URL").map(String::as_str),
            Some("https://example.com/thing"),
            "a URL is not a path list, so nothing about it is trimmed"
        );
        assert_eq!(
            environment.variables.get("SOME_SPEC").map(String::as_str),
            Some("registry=https://example.com")
        );
    }

    #[test]
    fn a_value_that_is_one_absent_path_is_withheld() {
        let steps = vec![step(
            "dev",
            &[("SSL_CERT_FILE", "/etc/ssl/certs/ca-bundle.crt")],
            &[],
        )];
        let environment = merge(&steps).expect("environment");
        assert!(
            !environment.variables.contains_key("SSL_CERT_FILE"),
            "a lone path the sandbox cannot see names something absent"
        );
    }

    #[test]
    fn a_later_step_overrides_an_earlier_one() {
        let steps = vec![
            step(
                "first",
                &[("RUSTFLAGS", "-C debuginfo=1")],
                &["/nix/store/abc"],
            ),
            step("second", &[("RUSTFLAGS", "-C debuginfo=2")], &[]),
        ];
        let environment = merge(&steps).expect("environment");
        assert_eq!(
            environment.variables.get("RUSTFLAGS").map(String::as_str),
            Some("-C debuginfo=2")
        );
        assert_eq!(environment.workspace, WORKSPACE);
    }

    #[test]
    fn a_credential_shaped_variable_is_carried_when_its_value_is_granted() {
        let steps = vec![step(
            "dev",
            &[
                ("GITHUB_TOKEN", "hunter2"),
                ("CARGO_HOME", "/nix/store/abc/cargo"),
            ],
            &["/nix/store/abc"],
        )];
        let environment = merge(&steps).expect("environment");
        assert_eq!(
            environment.variables.get("GITHUB_TOKEN").map(String::as_str),
            Some("hunter2"),
            "the backend does not guess what a variable is for"
        );
        assert_eq!(
            environment.variables.get("CARGO_HOME").map(String::as_str),
            Some("/nix/store/abc/cargo")
        );
    }

    #[test]
    fn a_value_naming_an_ungranted_path_keeps_the_granted_entries() {
        let steps = vec![step(
            "dev",
            &[("PKG_CONFIG_PATH", "/opt/secret:/nix/store/abc/lib")],
            &["/nix/store/abc"],
        )];
        let environment = merge(&steps).expect("environment");
        assert_eq!(
            environment
                .variables
                .get("PKG_CONFIG_PATH")
                .map(String::as_str),
            Some("/nix/store/abc/lib")
        );
    }

    #[test]
    fn a_value_inside_the_workspace_is_kept() {
        let steps = vec![step(
            "dev",
            &[("CARGO_HOME", "/src/.cargo")],
            &["/nix/store/abc"],
        )];
        let environment = merge(&steps).expect("environment");
        assert_eq!(
            environment.variables.get("CARGO_HOME").map(String::as_str),
            Some("/src/.cargo")
        );
    }

    #[test]
    fn a_value_inside_a_granted_path_is_kept() {
        let steps = vec![step(
            "dev",
            &[("PKG_CONFIG_PATH", "/nix/store/abc/lib")],
            &["/nix/store/abc"],
        )];
        let environment = merge(&steps).expect("environment");
        assert_eq!(
            environment
                .variables
                .get("PKG_CONFIG_PATH")
                .map(String::as_str),
            Some("/nix/store/abc/lib")
        );
    }

    #[test]
    fn the_backend_owns_home_tmpdir_and_pwd() {
        let steps = vec![step(
            "dev",
            &[("PATH", "/nix/store/abc/bin")],
            &["/nix/store/abc"],
        )];
        let environment = merge(&steps).expect("environment");
        assert_eq!(
            environment.variables.get("HOME").map(String::as_str),
            Some("/home/litterbox")
        );
        assert_eq!(
            environment.variables.get("TMPDIR").map(String::as_str),
            Some("/tmp")
        );
        assert_eq!(
            environment.variables.get("PWD").map(String::as_str),
            Some(WORKSPACE)
        );
        assert_eq!(
            environment.variables.get("PATH").map(String::as_str),
            Some("/nix/store/abc/bin"),
            "a contributed PATH is the sandbox's PATH, unchanged"
        );
        for name in ["HOME", "TMPDIR", "PWD"] {
            assert!(
                environment
                    .provenance
                    .variables
                    .iter()
                    .any(|variable| variable.name == name && variable.change == Change::Set),
                "{name} is not recorded as set"
            );
        }
    }

    #[test]
    fn no_step_means_no_path() {
        let environment = merge(&[]).expect("environment");
        assert_eq!(environment.variables.len(), 3);
        assert!(
            !environment.variables.contains_key("PATH"),
            "the backend does not invent a search path"
        );
        assert_eq!(environment.provenance.origin, Origin::Default);
    }

    #[test]
    fn an_unmounted_path_element_is_trimmed_not_dropped() {
        let steps = vec![step(
            "dev",
            &[("PATH", "/nix/store/abc/bin:/opt/not-granted/bin")],
            &["/nix/store/abc"],
        )];
        let environment = merge(&steps).expect("environment");
        assert_eq!(
            environment.variables.get("PATH").map(String::as_str),
            Some("/nix/store/abc/bin"),
            "the granted elements survive; only the unmounted one goes"
        );
        assert!(
            environment
                .provenance
                .variables
                .iter()
                .any(|variable| variable.name == "PATH" && variable.change == Change::Set),
            "the trimmed value is recorded"
        );
    }

    #[test]
    fn a_path_with_nothing_usable_is_withheld() {
        let steps = vec![step("dev", &[("PATH", "/opt/one:/opt/two")], &[])];
        let environment = merge(&steps).expect("environment");
        assert!(!environment.variables.contains_key("PATH"));
    }

    #[test]
    fn a_host_step_is_recorded_in_provenance() {
        let steps = vec![step("dev", &[], &["/nix/store/abc"])];
        let environment = merge(&steps).expect("environment");
        assert_eq!(environment.provenance.steps.len(), 1);
        assert_eq!(environment.provenance.steps[0].identity, "dev");
    }
}
