//! Preparing and checking the `devshell` kind.

use crate::sandbox::contract::{Contributions, ProvisionStep};
use crate::sandbox::environment::StepProvenance;

use super::{CheckContext, CheckReport, PrepareContext, PrepareError};

/// The installable a devshell step names.
fn installable(step: &ProvisionStep) -> Result<&str, PrepareError> {
    step.settings
        .get("installable")
        .and_then(toml::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            PrepareError::new(
                "the devshell step has no installable",
                "set installable to a flake or path",
            )
        })
}

/// Check the host and the store view without running the hook.
pub fn check(step: &ProvisionStep, ctx: &CheckContext<'_>) -> Result<CheckReport, PrepareError> {
    let installable = installable(step)?;
    let requirements = super::checks::check_host(installable, ctx.runner);
    let closure = if requirements.ok() {
        match &requirements.output_path {
            Some(path) => super::closure::closure(path, ctx.store_view, ctx.runner),
            None => Err(PrepareError::new(
                "the devShell has no output path",
                "run nix develop on the host first",
            )),
        }
    } else {
        Err(PrepareError::new(
            "host requirements are not met",
            "run litterbox doctor to see the missing requirement",
        ))
    };
    Ok(CheckReport {
        requirements,
        closure,
        store_view: ctx.store_view,
        installable: Some(installable.to_string()),
    })
}

/// Prepare the environment and the store view for a devshell step.
pub fn prepare(
    step: &ProvisionStep,
    ctx: &PrepareContext<'_>,
) -> Result<Contributions, PrepareError> {
    let installable = installable(step)?;
    let requirements = super::checks::check_host(installable, ctx.runner);
    if !requirements.ok() {
        let failure = requirements
            .checks
            .iter()
            .find(|check| !check.ok)
            .expect("a failed requirement");
        return Err(PrepareError::new(
            failure.detail.clone(),
            failure
                .remedy
                .clone()
                .unwrap_or_else(|| "run litterbox doctor".to_string()),
        ));
    }

    let baseline = requirements.baseline.as_deref().ok_or_else(|| {
        PrepareError::new(
            "print-dev-env reported no baseline",
            "run nix develop on the host first",
        )
    })?;
    let output_path = requirements.output_path.as_deref().ok_or_else(|| {
        PrepareError::new(
            "the devShell has no output path",
            "run nix develop on the host first",
        )
    })?;

    let mut contributions =
        super::environment::capture(installable, ctx.project_root, baseline, ctx.runner)?;
    contributions.mounts = super::closure::closure(output_path, ctx.store_view, ctx.runner)?;
    contributions.provenance.steps.push(StepProvenance {
        identity: step.identity.clone(),
        kind: step.kind.clone(),
        variables: contributions.provenance.variables.clone(),
    });
    Ok(contributions)
}
