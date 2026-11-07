//! Preparing and checking the `devshell` kind.

use crate::sandbox::contract::{Contributions, ProvisionStep};
use crate::sandbox::environment::StepProvenance;

use super::{CheckContext, CheckReport, HostRequirements, PrepareContext, PrepareError};

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

/// Check the host readiness without evaluating the installable.
pub fn check(step: &ProvisionStep, ctx: &CheckContext<'_>) -> Result<CheckReport, PrepareError> {
    let installable = installable(step)?;
    Ok(super::check(installable, ctx.store_view, ctx.runner))
}

/// Prepare the environment and the store view for a devshell step.
pub fn prepare(
    step: &ProvisionStep,
    ctx: &PrepareContext<'_>,
) -> Result<Contributions, PrepareError> {
    let installable = installable(step)?;
    refuse(&super::checks::check_readiness(ctx.runner))?;
    let evaluation = super::checks::check_installable(installable, ctx.runner);
    refuse(&evaluation)?;

    let baseline = evaluation.baseline.as_deref().ok_or_else(|| {
        PrepareError::new(
            "print-dev-env reported no baseline",
            "run nix develop on the host first",
        )
    })?;
    let output_path = evaluation.output_path.as_deref().ok_or_else(|| {
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

/// The first requirement that does not hold, as a preparation error.
fn refuse(requirements: &HostRequirements) -> Result<(), PrepareError> {
    match requirements.checks.iter().find(|check| !check.ok) {
        Some(failure) => Err(PrepareError::new(
            failure.detail.clone(),
            failure
                .remedy
                .clone()
                .unwrap_or_else(|| "run litterbox doctor".to_string()),
        )),
        None => Ok(()),
    }
}
