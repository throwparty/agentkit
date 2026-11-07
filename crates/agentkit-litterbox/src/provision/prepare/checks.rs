//! Host checks: what must exist before a devShell is prepared.

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

/// The host requirements and the devShell output path they found.
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

/// Check the host for a devShell, without realising it.
pub fn check_host(installable: &str, runner: &dyn CommandRunner) -> HostRequirements {
    let mut checks = Vec::new();

    let version = runner.run("nix", &["--version".to_string()], None, None);
    let (ok, detail) = match &version {
        Ok(output) if output.success() => (true, output.stdout_text().trim().to_string()),
        Ok(output) => (false, output.stderr_text().trim().to_string()),
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
    let (ok, detail, path, baseline) = match &json {
        Ok(output) if output.success() => match output_path(&output.stdout) {
            Some(path) => (true, path.clone(), Some(path), Some(output.stdout.clone())),
            None => (
                false,
                "no out variable in the devShell".to_string(),
                None,
                Some(output.stdout.clone()),
            ),
        },
        Ok(output) => (false, output.first_line(), None, None),
        Err(error) => (false, error.clone(), None, None),
    };

    if !ok && detail.contains("flakes") {
        checks.push(Check {
            name: "flakes enabled",
            ok: false,
            detail,
            remedy: Some(
                "add 'flakes' to experimental-features in the host's nix.conf".to_string(),
            ),
        });
        return HostRequirements {
            checks,
            output_path: None,
            baseline: None,
        };
    }

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

    let Some(path) = path else {
        return HostRequirements {
            checks,
            output_path: None,
            baseline,
        };
    };

    let valid = runner.run("nix", &["path-info".to_string(), path.clone()], None, None);
    let (ok, detail) = match &valid {
        Ok(output) if output.success() => (true, path.clone()),
        Ok(output) => (false, output.first_line()),
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
        output_path: Some(path),
        baseline,
    }
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
