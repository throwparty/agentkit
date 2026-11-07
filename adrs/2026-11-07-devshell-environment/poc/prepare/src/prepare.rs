//! Prepare: evaluate the devShell once, enter it under a sanitised caller, and
//! take its environment.
//!
//! The rule is nix-direnv's, and it is the one T-030 records: what the entered
//! shell sets is the devShell's environment, minus the shell's own state and the
//! values the backend supplies. Nothing else is dropped, so a variable that
//! `print-dev-env` marks exported but empty is still contributed with the value
//! the shell gives it.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use crate::checks::output_path;
use crate::failure::Failure;
use crate::model::{Contributions, Settings, Step};
use crate::util::{
    Caller, Records, argv, private_dir, read_nul, run, write_nul_strings, write_private,
};

/// Names the shell sets about itself; they say nothing about the devShell.
const SHELL_STATE: [&str; 5] = ["PWD", "OLDPWD", "SHLVL", "_", "HOME"];

/// The six names T-030 restores, because their entered values name host scratch
/// paths and a host terminal database the sandbox does not have.
const RESTORED: [&str; 6] = [
    "NIX_BUILD_TOP",
    "TMP",
    "TMPDIR",
    "TEMP",
    "TEMPDIR",
    "terminfo",
];

/// Names that carry a route to the caller's credentials or session.
const CREDENTIALS: [&str; 5] = [
    "SSH_AUTH_SOCK",
    "BITWARDEN_SSH_AUTH_SOCK",
    "DOCKER_HOST",
    "DBUS_SESSION_BUS_ADDRESS",
    "WAYLAND_DISPLAY",
];

/// Whether the project has consented to running its code on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consent {
    Allowed,
    Refused,
}

/// What preparation produced, and what it measured doing it.
pub struct Preparation {
    pub contributions: Contributions,
    pub provenance: String,
    pub measurements: Measurements,
    pub output_path: String,
    pub gc_root: bool,
}

#[derive(Debug, Default)]
pub struct Measurements {
    pub baseline_variables: usize,
    pub contributed_variables: usize,
    pub added: usize,
    pub overridden: usize,
    pub dropped: usize,
    pub rejected_records: usize,
    pub print_dev_env_ms: u128,
    pub entered_devshell_ms: u128,
}

impl Measurements {
    pub fn rendered(&self) -> String {
        format!(
            "baseline_variables={}\ncontributed_variables={}\nadded={}\noverridden={}\ndropped={}\nrejected_records={}\nprint_dev_env_ms={}\nentered_devshell_ms={}\n",
            self.baseline_variables,
            self.contributed_variables,
            self.added,
            self.overridden,
            self.dropped,
            self.rejected_records,
            self.print_dev_env_ms,
            self.entered_devshell_ms
        )
    }

    pub fn report(&self) -> String {
        format!(
            "baseline variables: {}\ncontributed variables: {}\nadded: {}\noverridden: {}\ndropped: {}\nrejected records: {}\nprint-dev-env: {} ms\nentered devShell: {} ms",
            self.baseline_variables,
            self.contributed_variables,
            self.added,
            self.overridden,
            self.dropped,
            self.rejected_records,
            self.print_dev_env_ms,
            self.entered_devshell_ms
        )
    }
}

/// The devShell a step names, or a refusal saying which setting is wrong.
pub fn installable(step: &Step) -> Result<&str, Failure> {
    match &step.settings {
        Settings::Devshell { installable } => Ok(installable),
        Settings::Command { .. } => Err(Failure::misconfigured(format!(
            "step '{}' is a {} step and prepares no environment",
            step.identity,
            step.kind()
        ))
        .with_step(&step.identity, step.kind())),
    }
}

/// Refuse a step whose project has not consented to running its code here.
///
/// This is the single gate both `prepare` and `run` use, so nothing evaluates a
/// project before the refusal.
pub fn consent_gate(step: &Step, consent: Consent) -> Result<(), Failure> {
    if consent == Consent::Allowed {
        return Ok(());
    }
    let installable = installable(step)?;
    Err(Failure::denied(format!(
        "step '{}' would evaluate '{installable}' and run its shell hook on this host, and the project has not consented",
        step.identity
    ))
    .with_step(&step.identity, step.kind())
    .with_remedy("add the project to allowed-projects, or set allow-project-setup"))
}

pub fn prepare(
    step: &Step,
    project_root: &Path,
    out: &Path,
    nix: &Path,
    caller: &Caller,
    consent: Consent,
) -> Result<Preparation, Failure> {
    let installable = installable(step)?;
    consent_gate(step, consent)?;

    private_dir(out)?;

    let started = Instant::now();
    let dev = run(
        nix,
        &argv(&["print-dev-env", installable, "--json"]),
        caller,
        Some(project_root),
    )
    .map_err(|failure| failure.with_step(&step.identity, step.kind()))?;
    if !dev.success() {
        return Err(Failure::unavailable(
            format!("nix print-dev-env {installable}"),
            "the installable does not evaluate",
        )
        .with_step(&step.identity, step.kind())
        .with_output(dev.stderr_excerpt())
        .with_remedy(format!("run nix print-dev-env {installable} on the host")));
    }
    let baseline = baseline(&dev.stdout)
        .map_err(|failure| failure.with_step(&step.identity, step.kind()))?;
    let output = output_path(&dev.stdout).ok_or_else(|| {
        Failure::misconfigured(format!(
            "step '{}': '{installable}' evaluates to something with no devShell environment",
            step.identity
        ))
        .with_step(&step.identity, step.kind())
        .with_remedy("point the step at a flake output that has a devShell")
    })?;
    let print_dev_env_ms = started.elapsed().as_millis();

    // Validity is a store query, and the answer decides whether entering the
    // shell can build anything. An unrealised devShell is refused here, before
    // the hook runs, and nothing is realised to find out.
    let valid = run(nix, &argv(&["path-info", &output]), caller, None)
        .map_err(|failure| failure.with_step(&step.identity, step.kind()))?;
    if !valid.success() {
        return Err(Failure::unavailable(
            format!("nix path-info {output}"),
            format!("the devShell for '{installable}' is not valid in the store"),
        )
        .with_step(&step.identity, step.kind())
        .with_output(valid.stderr_excerpt())
        .with_remedy(format!("run nix develop {installable} on the host first")));
    }

    // A root held for the lifetime of the sandbox would live in the record; the
    // prototype holds the output path, whose references are its closure.
    // Preparation is fail-closed: an unrooted closure can be collected between
    // the query and the mount, so no root means no preparation.
    hold_root(nix, caller, out, &output)?;
    let gc_root = true;

    let capture = out.join("entered.nul");
    let started = Instant::now();
    let enter = run(
        nix,
        &argv(&[
            "develop",
            installable,
            "--command",
            "sh",
            "-c",
            // The destination is data, never shell source.
            "env -0 > \"$1\"",
            "sh",
            &capture.display().to_string(),
        ]),
        caller,
        Some(project_root),
    )
    .map_err(|failure| failure.with_step(&step.identity, step.kind()))?;
    if !enter.success() {
        let mut failure = Failure::failed(format!(
            "the shell hook of '{}' exited non-zero",
            step.identity
        ))
        .with_step(&step.identity, step.kind())
        .with_output(enter.stderr_excerpt());
        if let Some(code) = enter.status.code() {
            failure = failure.with_exit(code);
        }
        return Err(failure.with_remedy("fix the shellHook, then create again"));
    }
    let entered = read_nul(&capture).map_err(|failure| {
        failure
            .with_step(&step.identity, step.kind())
            .with_remedy("remove whatever writes to the capture file")
    })?;
    let entered_devshell_ms = started.elapsed().as_millis();

    let contributed = contribute(&entered, &baseline);
    refuse_credentials(step, &contributed.environment)?;

    let environment = contributed.environment.clone();
    write_nul_strings(
        &out.join("env.nul"),
        environment.iter().map(|(k, v)| format!("{k}={v}")),
    )?;
    write_private(&out.join("out"), &output)?;
    write_private(
        &out.join("manifest"),
        &format!("installable={installable}\noutput_path={output}\n"),
    )?;

    let measurements = Measurements {
        baseline_variables: baseline.exported.len(),
        contributed_variables: environment.len(),
        added: contributed.added.len(),
        overridden: contributed.overridden.len(),
        dropped: contributed.dropped.len(),
        rejected_records: entered.rejected.len(),
        print_dev_env_ms,
        entered_devshell_ms,
    };

    let provenance = provenance(step, installable, &contributed, &measurements, gc_root);
    write_private(&out.join("provenance.txt"), &provenance)?;
    write_private(&out.join("measurements.txt"), &measurements.rendered())?;

    Ok(Preparation {
        contributions: Contributions::new(environment, Vec::new()),
        provenance,
        measurements,
        output_path: output,
        gc_root,
    })
}

/// Hold a temporary root on the output path so a collection cannot take it.
///
/// The root is indirect, so removing the directory releases it; rooting the
/// output path protects its whole reference closure. An unrooted closure can be
/// collected between the query and the mount, so a root that cannot be held
/// refuses preparation rather than continuing unprotected.
fn hold_root(nix: &Path, caller: &Caller, out: &Path, output: &str) -> Result<(), Failure> {
    let root = out.join("gcroot");
    let _ = std::fs::remove_file(&root);
    let nix_store = nix.with_file_name("nix-store");
    if !nix_store.is_file() {
        return Err(Failure::unavailable(
            nix_store.display().to_string(),
            "nix-store is not beside the nix executable, so no root can be held on the closure",
        )
        .with_remedy("hold the root in the sandbox record instead"));
    }
    let rooted = run(
        &nix_store,
        &argv(&[
            "--add-root",
            &root.display().to_string(),
            "--indirect",
            "--realise",
            output,
        ]),
        caller,
        None,
    )?;
    if !rooted.success() {
        return Err(Failure::unavailable(
            "nix-store --add-root",
            "no root could be held on the devShell's output path",
        )
        .with_output(rooted.stderr_excerpt())
        .with_remedy("hold the root in the sandbox record instead"));
    }
    Ok(())
}

/// What the entered shell contributes, and why the rest did not.
pub struct Contributed {
    pub environment: BTreeMap<String, String>,
    pub dropped: Vec<String>,
    pub added: Vec<String>,
    pub overridden: Vec<String>,
    pub missing: Vec<String>,
}

/// The environment rule: everything the shell set, minus shell state and
/// backend-supplied names, including exported names whose value is empty.
pub fn contribute(entered: &Records, baseline: &Baseline) -> Contributed {
    let mut environment = BTreeMap::new();
    let mut dropped = Vec::new();
    for (name, value) in &entered.values {
        if SHELL_STATE.contains(&name.as_str()) || RESTORED.contains(&name.as_str()) {
            dropped.push(name.clone());
            continue;
        }
        environment.insert(name.clone(), value.clone());
    }

    let mut added = Vec::new();
    let mut overridden = Vec::new();
    for (name, value) in &environment {
        match baseline.exported.get(name) {
            None => added.push(name.clone()),
            Some(base) if base != value => overridden.push(name.clone()),
            Some(_) => {}
        }
    }
    let missing = baseline
        .exported
        .keys()
        .filter(|name| !entered.values.contains_key(*name))
        .cloned()
        .collect();

    Contributed {
        environment,
        dropped,
        added,
        overridden,
        missing,
    }
}

/// Refuse an environment that carries a route to the caller's credentials.
fn refuse_credentials(
    step: &Step,
    environment: &BTreeMap<String, String>,
) -> Result<(), Failure> {
    let leaked: Vec<&str> = CREDENTIALS
        .iter()
        .copied()
        .filter(|name| environment.contains_key(*name))
        .collect();
    if leaked.is_empty() {
        return Ok(());
    }
    Err(Failure::unsupported(format!(
        "step '{}': the devShell exports {}, which would give the sandbox a route to the caller's session",
        step.identity,
        leaked.join(", ")
    ))
    .with_step(&step.identity, step.kind())
    .with_remedy("unset it in the shellHook; the sandbox must not carry the caller's credentials"))
}

fn provenance(
    step: &Step,
    installable: &str,
    contributed: &Contributed,
    measurements: &Measurements,
    gc_root: bool,
) -> String {
    let mut provenance = String::new();
    provenance.push_str(&format!("step: {} ({})\n", step.identity, step.kind()));
    provenance.push_str(&format!("origin: nix print-dev-env {installable}\n"));
    provenance.push_str(&format!(
        "variables: {}\n",
        measurements.contributed_variables
    ));
    provenance.push_str(&format!("added: {}\n", contributed.added.join(", ")));
    provenance.push_str(&format!(
        "overridden: {}\n",
        contributed.overridden.join(", ")
    ));
    provenance.push_str(&format!(
        "missing from the entered shell: {}\n",
        contributed.missing.join(", ")
    ));
    provenance.push_str(&format!(
        "dropped as shell state or restored: {}\n",
        contributed.dropped.join(", ")
    ));
    provenance.push_str(&format!(
        "rejected capture records: {}\n",
        measurements.rejected_records
    ));
    provenance.push_str(&format!("store root held: {gc_root}\n"));
    provenance
}

/// The baseline `print-dev-env` reports, values included even when empty.
#[derive(Debug)]
pub struct Baseline {
    pub exported: BTreeMap<String, String>,
}

pub fn baseline(json: &[u8]) -> Result<Baseline, Failure> {
    let value: serde_json::Value = serde_json::from_slice(json).map_err(|error| {
        Failure::misconfigured(format!("print-dev-env did not return JSON: {error}"))
    })?;
    let variables = value
        .get("variables")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            Failure::misconfigured("print-dev-env reported no variables: not a devShell")
        })?;
    let mut exported = BTreeMap::new();
    for (name, entry) in variables {
        if entry.get("type").and_then(serde_json::Value::as_str) != Some("exported") {
            continue;
        }
        let value = entry
            .get("value")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        exported.insert(name.clone(), value.to_string());
    }
    Ok(Baseline { exported })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::failure::Class;

    fn baseline_of(variables: &[(&str, &str)]) -> Baseline {
        Baseline {
            exported: variables
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
        }
    }

    fn entered_of(variables: &[(&str, &str)]) -> Records {
        Records {
            values: variables
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
            rejected: Vec::new(),
        }
    }

    fn step() -> Step {
        Step {
            identity: "dev".to_string(),
            settings: Settings::Devshell {
                installable: "./nix".to_string(),
            },
        }
    }

    #[test]
    fn an_exported_but_empty_variable_is_contributed() {
        let baseline = baseline_of(&[("PATH", "/bin"), ("patches", "")]);
        let entered = entered_of(&[("PATH", "/bin"), ("patches", "")]);
        let contributed = contribute(&entered, &baseline);
        assert_eq!(
            contributed.environment.get("patches").map(String::as_str),
            Some(""),
            "T-029 records these as contributed, not dropped"
        );
        assert!(contributed.dropped.is_empty());
        assert!(!contributed.added.contains(&"patches".to_string()));
    }

    #[test]
    fn shell_state_and_restored_names_are_dropped() {
        let baseline = baseline_of(&[("PATH", "/bin")]);
        let entered = entered_of(&[
            ("PATH", "/bin"),
            ("PWD", "/work"),
            ("SHLVL", "1"),
            ("_", "/bin/sh"),
            ("HOME", "/root"),
            ("NIX_BUILD_TOP", "/tmp/nix-shell.abc"),
            ("TMPDIR", "/tmp"),
            ("TEMP", "/tmp"),
            ("terminfo", "/nix/store/abc-ncurses"),
        ]);
        let contributed = contribute(&entered, &baseline);
        assert_eq!(contributed.environment.len(), 1);
        let mut dropped = contributed.dropped.clone();
        dropped.sort();
        assert_eq!(
            dropped,
            vec![
                "HOME",
                "NIX_BUILD_TOP",
                "PWD",
                "SHLVL",
                "TEMP",
                "TMPDIR",
                "_",
                "terminfo"
            ]
        );
    }

    #[test]
    fn a_refused_consent_names_the_step_and_the_installable() {
        let failure = consent_gate(&step(), Consent::Refused).unwrap_err();
        assert_eq!(failure.class, Class::Denied);
        assert!(failure.reason.contains("./nix"), "{}", failure.reason);
        assert_eq!(failure.step.as_deref(), Some("dev"));
    }

    #[test]
    fn an_allowed_consent_passes_the_gate() {
        assert!(consent_gate(&step(), Consent::Allowed).is_ok());
    }

    #[test]
    fn a_build_phase_variable_the_shell_sets_is_contributed() {
        let baseline = baseline_of(&[("strictDeps", "")]);
        let entered = entered_of(&[("strictDeps", "")]);
        let environment = contribute(&entered, &baseline).environment;
        assert!(environment.contains_key("strictDeps"));
    }

    #[test]
    fn added_and_overridden_are_named_against_the_baseline() {
        let baseline = baseline_of(&[("PATH", "/bin"), ("NIX_LDFLAGS", "-L/old")]);
        let entered = entered_of(&[
            ("PATH", "/nix/store/x/bin:/bin"),
            ("NIX_LDFLAGS", "-L/new"),
            ("CARGO_HOME", "/work/.cargo"),
        ]);
        let contributed = contribute(&entered, &baseline);
        assert_eq!(contributed.added, vec!["CARGO_HOME"]);
        assert_eq!(contributed.overridden, vec!["NIX_LDFLAGS", "PATH"]);
    }

    #[test]
    fn a_baseline_name_the_shell_lost_is_reported_missing() {
        let baseline = baseline_of(&[("PATH", "/bin"), ("TERM", "xterm")]);
        let entered = entered_of(&[("PATH", "/bin")]);
        assert_eq!(contribute(&entered, &baseline).missing, vec!["TERM"]);
    }

    #[test]
    fn a_credential_name_in_the_environment_is_refused() {
        let mut environment = BTreeMap::new();
        environment.insert("SSH_AUTH_SOCK".to_string(), "/run/agent.sock".to_string());
        let failure = refuse_credentials(&step(), &environment).unwrap_err();
        assert_eq!(failure.class, Class::Unsupported);
        assert!(failure.reason.contains("SSH_AUTH_SOCK"), "{}", failure.reason);
        assert_eq!(failure.step.as_deref(), Some("dev"));
    }

    #[test]
    fn an_environment_without_a_credential_name_is_accepted() {
        let mut environment = BTreeMap::new();
        environment.insert("PATH".to_string(), "/bin".to_string());
        assert!(refuse_credentials(&step(), &environment).is_ok());
    }

    #[test]
    fn a_command_step_prepares_no_environment() {
        let step = Step {
            identity: "fetch".to_string(),
            settings: Settings::Command {
                run: "true".to_string(),
            },
        };
        let failure = installable(&step).unwrap_err();
        assert_eq!(failure.class, Class::Misconfigured);
    }

    #[test]
    fn the_baseline_keeps_exported_names_whose_value_is_empty() {
        let json = br#"{"variables": {"PATH": {"type": "exported", "value": "/bin"}, "patches": {"type": "exported", "value": ""}, "shellHook": {"type": "var", "value": "echo"}}}"#;
        let baseline = baseline(json).expect("parses");
        assert_eq!(baseline.exported.get("PATH").map(String::as_str), Some("/bin"));
        assert_eq!(baseline.exported.get("patches").map(String::as_str), Some(""));
        assert!(!baseline.exported.contains_key("shellHook"));
    }

    #[test]
    fn a_json_without_variables_is_refused() {
        let failure = baseline(b"{}").unwrap_err();
        assert_eq!(failure.class, Class::Misconfigured);
    }

    #[test]
    fn the_provenance_carries_names_and_never_a_value() {
        let baseline = baseline_of(&[("PATH", "/bin")]);
        let entered = entered_of(&[("PATH", "/bin"), ("TOKEN", "fixture-secret")]);
        let contributed = contribute(&entered, &baseline);
        let measurements = Measurements::default();
        let rendered = provenance(&step(), "./nix", &contributed, &measurements, true);
        assert!(rendered.contains("TOKEN"));
        assert!(!rendered.contains("fixture-secret"), "{rendered}");
        assert!(!rendered.contains("/bin"), "{rendered}");
    }

    #[test]
    fn the_measurements_carry_counts_and_no_value() {
        let measurements = Measurements {
            baseline_variables: 85,
            contributed_variables: 91,
            added: 4,
            overridden: 4,
            dropped: 9,
            rejected_records: 1,
            print_dev_env_ms: 500,
            entered_devshell_ms: 600,
        };
        let rendered = measurements.rendered();
        assert!(rendered.contains("contributed_variables=91"), "{rendered}");
        assert!(rendered.contains("rejected_records=1"), "{rendered}");
    }
}
