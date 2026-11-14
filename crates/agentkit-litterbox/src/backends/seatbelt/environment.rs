//! The environment a sandbox command receives.
//!
//! Steps' contributions merge in order. A value that names an absolute path
//! outside the workspace and the grants is withheld, and the backend owns
//! `HOME`, `TMPDIR` and `PWD`. Nothing here writes a value to disk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::backends::environment::{Verdict, verdict};
use crate::sandbox::contract::{Error, ResolvedStep};
use crate::sandbox::environment::{
    Change, Environment, Origin, Provenance, StepProvenance, Variable,
};

/// Merge the resolved steps into the environment a sandbox receives.
pub(crate) fn merge(
    steps: &[ResolvedStep],
    workspace: &Path,
    granted: &[PathBuf],
) -> Result<Environment, Error> {
    let workspace_text = workspace.to_string_lossy().into_owned();
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
        let merged = contributions.environment_for(&workspace_text);
        for (name, value) in merged.variables {
            variables.insert(name, value);
        }
        provenance.steps.push(StepProvenance {
            identity: step.identity.clone(),
            kind: step.kind.clone(),
            variables: contributions.provenance.variables.clone(),
        });
        provenance
            .host_paths
            .extend(contributions.provenance.host_paths.clone());
    }

    let mut withheld = Vec::new();
    let mut trimmed = Vec::new();
    for (name, value) in &variables {
        match verdict(name, value, &workspace_text, granted) {
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

    let temporary = workspace.join("tmp");
    std::fs::create_dir_all(&temporary).map_err(|error| {
        Error::failed(format!(
            "cannot create the sandbox temporary directory '{}': {error}",
            temporary.display()
        ))
    })?;

    let owned = [
        ("HOME", workspace.to_path_buf()),
        ("TMPDIR", temporary),
        ("PWD", workspace.to_path_buf()),
    ];
    for (name, value) in owned {
        variables.insert(name.to_string(), value.to_string_lossy().into_owned());
        provenance.variables.push(Variable {
            name: name.to_string(),
            change: Change::Set,
        });
    }

    Ok(Environment {
        variables,
        provenance,
        workspace: workspace_text,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::merge;
    use crate::sandbox::contract::{Contributions, Placement, ResolvedStep};
    use crate::sandbox::environment::{Change, Origin, Provenance, Variable};

    fn workspace() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn step(identity: &str, environment: &[(&str, &str)], mounts: &[&str]) -> ResolvedStep {
        let contributions = Contributions {
            environment: environment
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            mounts: mounts.iter().map(PathBuf::from).collect(),
            provenance: Provenance {
                origin: Origin::Captured {
                    from: "print-dev-env".to_string(),
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
            },
            ..Contributions::default()
        };
        ResolvedStep {
            identity: identity.to_string(),
            kind: "devshell".to_string(),
            placement: Placement::Host,
            sandbox: None,
            contributions,
        }
    }

    #[test]
    fn a_devshell_path_keeps_its_granted_entries() {
        let ws = workspace();
        let steps = vec![step(
            "dev",
            &[("PATH", "/nix/store/abc/bin:/run/current-system/sw/bin")],
            &["/nix/store/abc"],
        )];
        let granted = vec![PathBuf::from("/nix/store/abc")];
        let environment = merge(&steps, ws.path(), &granted).expect("environment");
        assert_eq!(
            environment.variables.get("PATH").map(String::as_str),
            Some("/nix/store/abc/bin"),
            "an ungranted element is dropped, not the whole search path"
        );
    }

    #[test]
    fn a_later_step_overrides_an_earlier_one() {
        let ws = workspace();
        let steps = vec![
            step("first", &[("RUSTFLAGS", "-C debuginfo=1")], &[]),
            step("second", &[("RUSTFLAGS", "-C debuginfo=2")], &[]),
        ];
        let environment = merge(&steps, ws.path(), &[]).expect("environment");
        assert_eq!(
            environment.variables.get("RUSTFLAGS").map(String::as_str),
            Some("-C debuginfo=2")
        );
    }

    #[test]
    fn a_value_naming_an_ungranted_path_is_withheld() {
        let ws = workspace();
        let steps = vec![step(
            "dev",
            &[("PKG_CONFIG_PATH", "/opt/secret:/usr/lib")],
            &[],
        )];
        let environment = merge(&steps, ws.path(), &[]).expect("environment");
        assert!(!environment.variables.contains_key("PKG_CONFIG_PATH"));
        assert!(environment.provenance.variables.iter().any(|variable| {
            variable.name == "PKG_CONFIG_PATH" && variable.change == Change::Removed
        }));
    }

    #[test]
    fn a_value_inside_the_workspace_or_a_grant_is_kept() {
        let ws = workspace();
        let tool = ws.path().join("tool");
        std::fs::create_dir_all(&tool).expect("tool");
        let steps = vec![step(
            "dev",
            &[
                ("CARGO_HOME", &format!("{}/cargo", ws.path().display())),
                ("LD_LIBRARY_PATH", &format!("{}/lib", tool.display())),
            ],
            &[],
        )];
        let environment =
            merge(&steps, ws.path(), std::slice::from_ref(&tool)).expect("environment");
        let cargo = format!("{}/cargo", ws.path().display());
        let library = format!("{}/lib", tool.display());
        assert_eq!(
            environment.variables.get("CARGO_HOME").map(String::as_str),
            Some(cargo.as_str())
        );
        assert_eq!(
            environment
                .variables
                .get("LD_LIBRARY_PATH")
                .map(String::as_str),
            Some(library.as_str())
        );
    }

    #[test]
    fn the_backend_owns_home_tmpdir_and_pwd() {
        let ws = workspace();
        let steps = vec![step("dev", &[("PATH", "/usr/bin")], &[])];
        let environment = merge(&steps, ws.path(), &[]).expect("environment");
        assert_eq!(
            environment.variables.get("HOME").map(String::as_str),
            Some(ws.path().to_string_lossy().as_ref())
        );
        assert_eq!(
            environment.variables.get("TMPDIR").map(String::as_str),
            Some(ws.path().join("tmp").to_string_lossy().as_ref())
        );
        assert_eq!(
            environment.variables.get("PWD").map(String::as_str),
            Some(ws.path().to_string_lossy().as_ref())
        );
        assert!(ws.path().join("tmp").is_dir());
        for name in ["HOME", "TMPDIR", "PWD"] {
            assert!(
                environment
                    .provenance
                    .variables
                    .iter()
                    .any(|variable| variable.name == name && variable.change == Change::Set)
            );
        }
    }

    #[test]
    fn no_step_leaves_only_the_three_owned_variables() {
        let ws = workspace();
        let environment = merge(&[], ws.path(), &[]).expect("environment");
        assert_eq!(environment.variables.len(), 3);
        assert!(!environment.variables.contains_key("PATH"));
        assert_eq!(environment.provenance.origin, Origin::Default);
    }

    #[test]
    fn a_captured_origin_and_step_provenance_are_kept() {
        let ws = workspace();
        let steps = vec![step("dev", &[("RUSTFLAGS", "-C debuginfo=2")], &[])];
        let environment = merge(&steps, ws.path(), &[]).expect("environment");
        assert!(matches!(
            environment.provenance.origin,
            Origin::Captured { .. }
        ));
        assert_eq!(environment.provenance.steps.len(), 1);
        assert_eq!(environment.provenance.steps[0].identity, "dev");
    }
}
