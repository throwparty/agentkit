//! Host checks: what must exist before a devShell is prepared.
//!
//! Readiness is checked without an installable: nix, flakes and a store that
//! answers. The installable is evaluated only during create, and only after a
//! non-realising store query proves its devShell is already realised.

use super::runner::CommandRunner;

/// One host requirement and its outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// The primitive or requirement.
    pub name: &'static str,
    /// Whether it holds.
    pub ok: bool,
    /// What was found.
    pub detail: String,
    /// What the user can do when it does not hold.
    pub remedy: Option<String>,
}

/// The host requirements and, when create evaluated it, the devShell output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostRequirements {
    /// The checks, in the order a failure should be read.
    pub checks: Vec<Check>,
    /// The devShell's output path, when evaluation found one.
    pub output_path: Option<String>,
    /// The raw `print-dev-env --json` output, when evaluation succeeded.
    pub baseline: Option<Vec<u8>>,
}

impl HostRequirements {
    /// Whether every requirement holds.
    pub fn ok(&self) -> bool {
        self.checks.iter().all(|check| check.ok)
    }
}

/// Check the readiness requirements. This never evaluates an installable.
pub fn check_readiness(runner: &dyn CommandRunner) -> HostRequirements {
    let mut checks = Vec::new();

    let version = runner.run("nix", &["--version".to_string()], None, None);
    let (ok, detail) = match &version {
        Ok(output) if output.success() => (true, output.stdout_text().trim().to_string()),
        Ok(output) => (false, output.first_line()),
        Err(error) => (false, error.clone()),
    };
    checks.push(Check {
        name: "nix",
        ok,
        detail,
        remedy: (!ok).then(|| "install nix and put it on PATH".to_string()),
    });
    if !ok {
        return HostRequirements {
            checks,
            output_path: None,
            baseline: None,
        };
    }

    let features = runner.run(
        "nix",
        &[
            "config".to_string(),
            "show".to_string(),
            "experimental-features".to_string(),
        ],
        None,
        None,
    );
    let (ok, detail) = match &features {
        Ok(output) if output.success() => {
            let enabled = output
                .stdout_text()
                .split_whitespace()
                .any(|feature| feature == "flakes");
            (
                enabled,
                if enabled {
                    "flakes".to_string()
                } else {
                    "flakes are not enabled".to_string()
                },
            )
        }
        Ok(output) => (false, output.first_line()),
        Err(error) => (false, error.clone()),
    };
    checks.push(Check {
        name: "flakes enabled",
        ok,
        detail,
        remedy: (!ok)
            .then(|| "add 'flakes' to experimental-features in the host's nix.conf".to_string()),
    });
    if !ok {
        return HostRequirements {
            checks,
            output_path: None,
            baseline: None,
        };
    }

    let store = runner.run(
        "nix",
        &["store".to_string(), "info".to_string()],
        None,
        None,
    );
    let (ok, detail) = match &store {
        Ok(output) if output.success() => (
            true,
            output
                .stdout_text()
                .lines()
                .next()
                .unwrap_or_default()
                .trim()
                .to_string(),
        ),
        Ok(output) => (false, output.first_line()),
        Err(error) => (false, error.clone()),
    };
    checks.push(Check {
        name: "store answers",
        ok,
        detail,
        remedy: (!ok).then(|| "check that the nix store or daemon is reachable".to_string()),
    });

    HostRequirements {
        checks,
        output_path: None,
        baseline: None,
    }
}

/// Check that the installable's devShell is realised, then read its baseline.
///
/// The devShell is named as a flake reference and resolved without building
/// anything. A dry run then names every derivation the host would still build;
/// if any is missing, the step refuses before `print-dev-env` runs, because
/// `print-dev-env` realises whatever its environment derivation needs.
pub fn check_installable(installable: &str, runner: &dyn CommandRunner) -> HostRequirements {
    let mut checks = Vec::new();

    let system = runner.run(
        "nix",
        &[
            "eval".to_string(),
            "--raw".to_string(),
            "--impure".to_string(),
            "--expr".to_string(),
            "builtins.currentSystem".to_string(),
        ],
        None,
        None,
    );
    let (ok, detail, system) = match &system {
        Ok(output) if output.success() => {
            let system = output.stdout_text().trim().to_string();
            (true, system.clone(), Some(system))
        }
        Ok(output) => (false, output.first_line(), None),
        Err(error) => (false, error.clone(), None),
    };
    checks.push(Check {
        name: "nix reports a system",
        ok,
        detail,
        remedy: (!ok)
            .then(|| "check that nix evaluates builtins.currentSystem on this host".to_string()),
    });
    let Some(system) = system else {
        return HostRequirements {
            checks,
            output_path: None,
            baseline: None,
        };
    };

    let reference = dev_shell_reference(installable, &system);

    let resolved = runner.run(
        "nix",
        &[
            "eval".to_string(),
            "--raw".to_string(),
            format!("{reference}.drvPath"),
        ],
        None,
        None,
    );
    let (ok, detail, derivation) = match &resolved {
        Ok(output) if output.success() => {
            let derivation = output.stdout_text().trim().to_string();
            (true, derivation.clone(), Some(derivation))
        }
        Ok(output) => (false, output.first_line(), None),
        Err(error) => (false, error.clone(), None),
    };
    checks.push(Check {
        name: "devShell derivation resolves",
        ok,
        detail,
        remedy: (!ok).then(|| format!("check that {installable} names a devShell")),
    });
    let Some(derivation) = derivation else {
        return HostRequirements {
            checks,
            output_path: None,
            baseline: None,
        };
    };

    // The dry run is the non-realising query: it names every derivation the
    // host would still have to materialise before the devShell is usable.
    let planned = runner.run(
        "nix",
        &["build".to_string(), "--dry-run".to_string(), reference],
        None,
        None,
    );
    let (ok, detail) = match &planned {
        Ok(output) if output.success() => {
            let unrealised = planned_builds(&output.stderr_text(), &derivation);
            if unrealised.is_empty() {
                (true, "nothing to build".to_string())
            } else {
                (
                    false,
                    format!("the host would build {}", unrealised.join(", ")),
                )
            }
        }
        Ok(output) => (false, output.last_line()),
        Err(error) => (false, error.clone()),
    };
    checks.push(Check {
        name: "devShell is realised in the store",
        ok,
        detail,
        remedy: (!ok).then(|| format!("run nix develop {installable} on the host first")),
    });
    if !ok {
        return HostRequirements {
            checks,
            output_path: None,
            baseline: None,
        };
    }

    // print-dev-env names the environment derivation, which only exists after
    // evaluation, so the name itself cannot be queried beforehand.
    let json = runner.run(
        "nix",
        &[
            "print-dev-env".to_string(),
            installable.to_string(),
            "--json".to_string(),
        ],
        None,
        None,
    );
    let (ok, detail, baseline, path) = match &json {
        Ok(output) if output.success() => match output_path(&output.stdout) {
            Some(path) => (true, path.clone(), Some(output.stdout.clone()), Some(path)),
            None => (
                false,
                "no out variable in the devShell".to_string(),
                Some(output.stdout.clone()),
                None,
            ),
        },
        Ok(output) => (false, output.first_line(), None, None),
        Err(error) => (false, error.clone(), None, None),
    };
    checks.push(Check {
        name: "installable evaluates",
        ok,
        detail,
        remedy: (!ok).then(|| format!("check {installable} evaluates on this host")),
    });

    if let Ok(output) = &json
        && output.stderr_text().contains("dirty")
    {
        checks.push(Check {
            name: "dirty tree",
            ok: true,
            detail: "print-dev-env may write a flake source path to the store; the closure may \
                     differ from a clean checkout"
                .to_string(),
            remedy: None,
        });
    }

    let Some(path) = path.filter(|_| ok) else {
        return HostRequirements {
            checks,
            output_path: None,
            baseline,
        };
    };

    let valid = runner.run("nix", &["path-info".to_string(), path.clone()], None, None);
    let (ok, detail) = match &valid {
        Ok(output) if output.success() => (true, path.clone()),
        Ok(output) => (false, output.last_line()),
        Err(error) => (false, error.clone()),
    };
    checks.push(Check {
        name: "devShell is valid in the store",
        ok,
        detail,
        remedy: (!ok).then(|| format!("run nix develop {installable} on the host first")),
    });

    HostRequirements {
        checks,
        output_path: ok.then_some(path),
        baseline,
    }
}

/// The flake reference that names the devShell an installable resolves to.
fn dev_shell_reference(installable: &str, system: &str) -> String {
    match installable.split_once('#') {
        Some((flake, name)) if !name.is_empty() => {
            format!("{flake}#devShells.{system}.{name}")
        }
        Some((flake, _)) => format!("{flake}#devShells.{system}.default"),
        None => format!("{installable}#devShells.{system}.default"),
    }
}

/// The derivations a dry run says the host would build, apart from `derivation`.
///
/// `nix build --dry-run` lists one derivation path per line under a "will be
/// built" heading; substitutions are reported as store paths, not derivations.
fn planned_builds(report: &str, derivation: &str) -> Vec<String> {
    report
        .lines()
        .map(str::trim)
        .filter(|line| line.ends_with(".drv"))
        .filter(|line| *line != derivation)
        .map(str::to_string)
        .collect()
}

/// The output path `print-dev-env` reports, if any.
fn output_path(json: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(json).ok()?;
    value
        .get("variables")?
        .get("out")?
        .get("value")?
        .as_str()
        .map(str::to_string)
}
