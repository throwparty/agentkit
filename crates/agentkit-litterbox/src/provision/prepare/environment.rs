//! Capturing the environment a devShell produces.
//!
//! The `print-dev-env` script is evaluated once in a sanitised bash. The
//! contribution is the difference against the caller, with the variables Nix
//! owns restored. Shell state and the caller's own variables never cross.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::sandbox::contract::Contributions;
use crate::sandbox::environment::{Change, HostPath, Origin, Provenance, Variable};

use super::PrepareError;
use super::runner::CommandRunner;

/// The variables Nix sets for its own shell and that nix-direnv restores.
const RESTORED: [&str; 6] = [
    "NIX_BUILD_TOP",
    "TMP",
    "TMPDIR",
    "TEMP",
    "TEMPDIR",
    "terminfo",
];

/// Variables that belong to the caller's shell, never to the devShell.
const SHELL_STATE: [&str; 5] = ["HOME", "PWD", "OLDPWD", "SHLVL", "_"];

/// The hook's output is diagnostics; a noisy hook must not fill memory.
const HOOK_OUTPUT_LIMIT: usize = 64 * 1024;

/// The `print-dev-env` baseline, used to report additions and overrides.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Baseline {
    /// The exported variables with a value.
    pub exported: BTreeMap<String, String>,
    /// Every variable's type, as `print-dev-env` reports it.
    pub types: BTreeMap<String, String>,
}

/// Parse the `print-dev-env --json` baseline.
pub fn baseline(json: &[u8]) -> Result<Baseline, PrepareError> {
    let value: serde_json::Value = serde_json::from_slice(json).map_err(|error| {
        PrepareError::new(
            format!("print-dev-env output is not JSON: {error}"),
            "check that nix print-dev-env succeeds on this host",
        )
    })?;
    let variables = value
        .get("variables")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            PrepareError::new(
                "print-dev-env reported no variables",
                "check that the installable is a devShell",
            )
        })?;

    let mut baseline = Baseline::default();
    for (name, entry) in variables {
        let kind = entry
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("var")
            .to_string();
        baseline.types.insert(name.clone(), kind.clone());
        if kind == "exported"
            && let Some(value) = entry.get("value").and_then(serde_json::Value::as_str)
            && !value.is_empty()
        {
            baseline.exported.insert(name.clone(), value.to_string());
        }
    }
    Ok(baseline)
}

/// Evaluate the `print-dev-env` script and return what it contributes.
pub fn capture(
    installable: &str,
    project_root: &Path,
    baseline_json: &[u8],
    runner: &dyn CommandRunner,
) -> Result<Contributions, PrepareError> {
    let baseline = baseline(baseline_json)?;

    let script = runner
        .run(
            "nix",
            &["print-dev-env".to_string(), installable.to_string()],
            None,
            Some(project_root),
        )
        .map_err(|error| {
            PrepareError::new(error, "check that nix print-dev-env succeeds on this host")
        })?;
    if !script.success() {
        return Err(PrepareError::new(
            script.first_line(),
            format!("check {installable} evaluates on this host"),
        ));
    }

    let temp = tempfile::tempdir().map_err(|error| {
        PrepareError::new(
            error.to_string(),
            "ensure the temporary directory is writable",
        )
    })?;
    let script_path = temp.path().join("print-dev-env.sh");
    let wrapper_path = temp.path().join("capture.sh");
    let before_path = temp.path().join("before.nul");
    let after_path = temp.path().join("after.nul");

    fs::write(&script_path, &script.stdout).map_err(|error| {
        PrepareError::new(
            error.to_string(),
            "ensure the temporary directory is writable",
        )
    })?;
    fs::write(
        &wrapper_path,
        "trap 'env -0 > \"$3\"' EXIT\n\
         env -0 > \"$2\"\n\
         source \"$1\"\n",
    )
    .map_err(|error| {
        PrepareError::new(
            error.to_string(),
            "ensure the temporary directory is writable",
        )
    })?;

    let mut caller = BTreeMap::new();
    caller.insert(
        "HOME".to_string(),
        std::env::var("HOME").unwrap_or_else(|_| "/".to_string()),
    );
    caller.insert(
        "PATH".to_string(),
        "/run/current-system/sw/bin:/usr/local/bin:/usr/bin:/bin".to_string(),
    );
    caller.insert(
        "OLDPWD".to_string(),
        project_root.to_string_lossy().into_owned(),
    );
    caller.insert("SHLVL".to_string(), "1".to_string());

    let output = runner
        .run_bounded(
            "bash",
            &[
                wrapper_path.to_string_lossy().into_owned(),
                script_path.to_string_lossy().into_owned(),
                before_path.to_string_lossy().into_owned(),
                after_path.to_string_lossy().into_owned(),
            ],
            Some(&caller),
            Some(project_root),
            HOOK_OUTPUT_LIMIT,
        )
        .map_err(|error| PrepareError::new(error, "check that bash is available on the host"))?;

    if !output.success() {
        let detail = output.stderr_text();
        let reason = detail
            .lines()
            .next()
            .unwrap_or("the shell hook failed")
            .trim()
            .to_string();
        return Err(PrepareError::new(
            reason,
            "fix the shell hook and try again",
        ));
    }
    if !after_path.exists() {
        return Err(PrepareError::new(
            "the shell hook replaced or exited the shell before the environment was captured",
            "remove the exec from the hook or the shell rc",
        ));
    }

    let before = read_nul(&before_path)?;
    let after = read_nul(&after_path)?;
    Ok(contribute(
        &before,
        &after,
        &baseline,
        project_root,
        installable,
    ))
}

/// Compute the contribution from the before and after environments.
fn contribute(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
    baseline: &Baseline,
    project_root: &Path,
    installable: &str,
) -> Contributions {
    let root = project_root.to_string_lossy();
    let mut environment = BTreeMap::new();
    for (name, value) in after {
        if RESTORED.contains(&name.as_str()) || SHELL_STATE.contains(&name.as_str()) {
            continue;
        }
        if before.get(name) == Some(value) {
            continue;
        }
        environment.insert(name.clone(), value.clone());
    }

    let mut variables = Vec::new();
    let mut host_paths = Vec::new();
    let mut workspace_variables = Vec::new();
    for (name, value) in &environment {
        let change = match baseline.exported.get(name) {
            Some(base) if base == value => Change::Unchanged,
            _ => Change::Set,
        };
        variables.push(Variable {
            name: name.clone(),
            change,
        });
        if value.contains(root.as_ref()) {
            workspace_variables.push(name.clone());
        }
        if is_single_absolute_path(value)
            && !value.starts_with("/nix/store")
            && !value.starts_with(root.as_ref())
        {
            host_paths.push(HostPath {
                variable: name.clone(),
                path: PathBuf::from(value),
            });
        }
    }
    workspace_variables.sort();

    Contributions {
        environment,
        provenance: Provenance {
            origin: Origin::Captured {
                from: format!("nix print-dev-env {installable}"),
            },
            variables,
            host_paths,
            steps: Vec::new(),
        },
        mounts: Vec::new(),
        workspace_root: Some(project_root.to_path_buf()),
        workspace_variables,
    }
}

/// A value that names one absolute path rather than a list or a flag.
fn is_single_absolute_path(value: &str) -> bool {
    !value.is_empty() && !value.contains(':') && Path::new(value).is_absolute()
}

/// Read a NUL-separated environment dump.
fn read_nul(path: &Path) -> Result<BTreeMap<String, String>, PrepareError> {
    let bytes = fs::read(path).map_err(|error| {
        PrepareError::new(
            error.to_string(),
            "ensure the temporary directory is writable",
        )
    })?;
    let mut map = BTreeMap::new();
    for record in bytes.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(record);
        if let Some((name, value)) = text.split_once('=')
            && !is_exported_function(name)
        {
            map.insert(name.to_string(), value.to_string());
        }
    }
    Ok(map)
}

/// Bash exports a function as a `BASH_FUNC_<name>%%` record, which a later Bash
/// would import. It is shell state, not an environment variable.
fn is_exported_function(name: &str) -> bool {
    name.starts_with("BASH_FUNC_") && name.ends_with("%%")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    fn baseline_fixture() -> Baseline {
        Baseline {
            exported: map(&[("PATH", "/usr/bin")]),
            types: map(&[("PATH", "exported")]),
        }
    }

    #[test]
    fn the_diff_keeps_empty_variables_and_drops_shell_state() {
        let before = map(&[
            ("HOME", "/home/user"),
            ("PATH", "/usr/bin"),
            ("PWD", "/repo"),
            ("OLDPWD", "/repo"),
            ("SHLVL", "1"),
            ("_", "/bin/bash"),
        ]);
        let after = map(&[
            ("HOME", "/home/user"),
            ("PATH", "/repo/.cargo/bin:/usr/bin"),
            ("PWD", "/repo"),
            ("OLDPWD", "/repo"),
            ("SHLVL", "1"),
            ("_", "/bin/bash"),
            ("CARGO_HOME", "/repo/.cargo"),
            ("RUSTUP_HOME", "/repo/.rustup"),
            ("RUSTFLAGS", "-L/repo/.cargo"),
            ("cmakeFlags", ""),
            ("patches", ""),
            ("NIX_BUILD_TOP", "/tmp/nix-shell.X"),
            ("TMPDIR", "/tmp/nix-shell.X"),
        ]);
        let contributions = contribute(
            &before,
            &after,
            &baseline_fixture(),
            Path::new("/repo"),
            "./nix",
        );

        assert_eq!(contributions.environment["cmakeFlags"], "");
        assert_eq!(contributions.environment["patches"], "");
        assert_eq!(
            contributions.environment["PATH"],
            "/repo/.cargo/bin:/usr/bin"
        );
        for name in [
            "HOME",
            "PWD",
            "OLDPWD",
            "SHLVL",
            "_",
            "NIX_BUILD_TOP",
            "TMPDIR",
        ] {
            assert!(
                !contributions.environment.contains_key(name),
                "{name} must not cross"
            );
        }
        assert!(
            contributions
                .workspace_variables
                .contains(&"CARGO_HOME".to_string())
        );
        assert!(
            contributions
                .workspace_variables
                .contains(&"PATH".to_string())
        );
        assert_eq!(
            contributions.provenance.origin,
            Origin::Captured {
                from: "nix print-dev-env ./nix".to_string()
            }
        );
    }

    #[test]
    fn a_host_path_is_reported_not_granted() {
        let after = map(&[("SSH_AUTH_SOCK", "/run/user/1000/ssh-agent.sock")]);
        let contributions = contribute(
            &BTreeMap::new(),
            &after,
            &Baseline::default(),
            Path::new("/repo"),
            "./nix",
        );
        assert_eq!(contributions.provenance.host_paths.len(), 1);
        assert_eq!(
            contributions.provenance.host_paths[0].variable,
            "SSH_AUTH_SOCK"
        );
        assert_eq!(
            contributions.provenance.host_paths[0].path,
            PathBuf::from("/run/user/1000/ssh-agent.sock")
        );
    }

    #[test]
    fn a_caller_value_is_not_contributed() {
        let before = map(&[("RUSTFLAGS", "caller")]);
        let after = map(&[("RUSTFLAGS", "caller")]);
        let contributions = contribute(
            &before,
            &after,
            &Baseline::default(),
            Path::new("/repo"),
            "./nix",
        );
        assert!(!contributions.environment.contains_key("RUSTFLAGS"));
    }

    #[test]
    fn an_exported_function_is_shell_state_not_a_variable() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("after.nul");
        let mut bytes = Vec::new();
        for record in ["TOKEN=secret", "BASH_FUNC_greet%%=() {  echo hi; }\n"] {
            bytes.extend_from_slice(record.as_bytes());
            bytes.push(0);
        }
        std::fs::write(&path, bytes).expect("write the fixture");

        let parsed = read_nul(&path).expect("parse the capture");
        assert_eq!(parsed.get("TOKEN").map(String::as_str), Some("secret"));
        assert!(
            !parsed.keys().any(|name| name.starts_with("BASH_FUNC_")),
            "an exported function must not become a contribution"
        );
    }
}
