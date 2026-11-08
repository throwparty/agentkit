//! Provisioning pieces the mechanisms share.
//!
//! The step runner, the create budget, the transfer deadline and the output
//! caps live here so the backends cannot drift apart. The runner leaves the
//! transport out: a backend passes a closure that runs one command and maps
//! its own error type.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;

use crate::sandbox::contract::{
    Error, Execution, ProvisioningClass, ProvisioningFailure, ResolvedStep,
};
use crate::sandbox::environment::StepProvenance;

/// The fixed ceiling for one create's provisioning, independent of step count.
pub(crate) const CREATE_BUDGET: Duration = Duration::from_secs(2 * 60 * 60);

/// How long a workspace upload may take before create is rolled back.
pub(crate) const UPLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How much output one provisioning step may buffer in the transport.
pub(crate) const PROVISION_OUTPUT_CAP: usize = 16 * 1024;

/// How much of a failing command's output travels in an error message.
pub(crate) const ERROR_OUTPUT: usize = 4096;

/// Apply a backend's ordered steps, keeping only the ones that completed.
///
/// A step without a sandbox action is skipped, which is how a host-prepared
/// step passes through. `on_error` maps the transport's own failure into the
/// contract, so each backend names its mechanism in the report.
pub(crate) fn run_steps<'a, E, F, M>(
    backend: &'a str,
    sandbox: &'a str,
    provision: &'a [ResolvedStep],
    last_step: &'a Mutex<Option<ResolvedStep>>,
    mut exec: F,
    on_error: M,
) -> BoxFuture<'a, Result<Vec<StepProvenance>, Error>>
where
    E: Send + 'a,
    F: FnMut(&'a str, &'a str) -> BoxFuture<'a, Result<Execution, E>> + Send + 'a,
    M: Fn(&ResolvedStep, E) -> Error + Send + Sync + 'a,
{
    Box::pin(async move {
        let mut steps = Vec::with_capacity(provision.len());
        for step in provision {
            let Some(action) = &step.sandbox else {
                continue;
            };
            let identity = &step.identity;
            *last_step
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = Some(step.clone());
            let started = Instant::now();
            tracing::info!(
                sandbox = %sandbox,
                step = %identity,
                kind = step.kind(),
                "starting provisioning step"
            );
            let result = exec(identity.as_str(), action.run.as_str())
                .await
                .map_err(|error| on_error(step, error))?;
            let elapsed_ms = started.elapsed().as_millis() as u64;
            if result.exit_code != 0 {
                let output = combined_output(&result.stdout, &result.stderr);
                tracing::warn!(
                    sandbox = %sandbox,
                    step = %identity,
                    kind = step.kind(),
                    exit_code = result.exit_code,
                    output_len = output.len(),
                    "the provisioning step failed"
                );
                return Err(command_failure(backend, step, result.exit_code, &output));
            }
            steps.push(StepProvenance {
                identity: identity.clone(),
                kind: step.kind().to_string(),
                variables: Vec::new(),
            });
            tracing::info!(
                sandbox = %sandbox,
                step = %identity,
                kind = step.kind(),
                exit_code = result.exit_code,
                elapsed_ms,
                "provisioning step finished"
            );
        }
        Ok(steps)
    })
}

/// A step that ran and exited non-zero.
pub(crate) fn command_failure(
    backend: &str,
    step: &ResolvedStep,
    exit_code: i32,
    output: &str,
) -> Error {
    let identity = step.identity();
    Error::Provisioning {
        failure: Box::new(ProvisioningFailure {
            class: ProvisioningClass::Failed,
            backend: Some(backend.to_string()),
            step: Some(identity.to_string()),
            kind: Some(step.kind().to_string()),
            reason: format!("provisioning step '{identity}' failed with exit code {exit_code}"),
            remedy: "inspect the step output, fix the command, and create again".to_string(),
            exit_code: Some(exit_code),
            output: Some(bounded_output(output)),
        }),
    }
}

/// Both streams, labelled, so a diagnostic on stdout is not lost behind a
/// warning on stderr.
pub(crate) fn combined_output(stdout: &str, stderr: &str) -> String {
    let mut output = String::new();
    if !stdout.is_empty() {
        output.push_str("stdout:\n");
        output.push_str(stdout);
    }
    if !stderr.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str("stderr:\n");
        output.push_str(stderr);
    }
    output
}

pub(crate) fn bounded_output(text: &str) -> String {
    if text.chars().count() <= ERROR_OUTPUT {
        return text.to_string();
    }
    let mut bounded: String = text.chars().take(ERROR_OUTPUT).collect();
    bounded.push_str("\n[truncated]");
    bounded
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use futures_util::future::BoxFuture;

    use super::{CREATE_BUDGET, ERROR_OUTPUT, bounded_output, combined_output, run_steps};
    use crate::sandbox::contract::{Error, Execution, ResolvedStep};

    fn step(identity: &str, run: &str) -> ResolvedStep {
        ResolvedStep {
            identity: identity.to_string(),
            kind: "command".to_string(),
            placement: crate::sandbox::contract::Placement::Sandbox,
            sandbox: Some(crate::sandbox::contract::SandboxAction {
                run: run.to_string(),
            }),
            contributions: crate::sandbox::contract::Contributions::default(),
        }
    }

    fn host_step(identity: &str) -> ResolvedStep {
        ResolvedStep {
            identity: identity.to_string(),
            kind: "devshell".to_string(),
            placement: crate::sandbox::contract::Placement::Host,
            sandbox: None,
            contributions: crate::sandbox::contract::Contributions::default(),
        }
    }

    #[tokio::test]
    async fn a_successful_step_is_recorded_and_host_steps_are_skipped() {
        let provision = vec![host_step("dev"), step("build", "make")];
        let last = Mutex::new(None);
        let steps = run_steps(
            "test",
            "demo",
            &provision,
            &last,
            |_identity, _run| {
                Box::pin(async {
                    Ok(Execution {
                        exit_code: 0,
                        stdout: String::new(),
                        stderr: String::new(),
                    })
                }) as BoxFuture<'_, Result<Execution, std::io::Error>>
            },
            |_step, error| Error::failed(error.to_string()),
        )
        .await
        .expect("steps run");

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].identity, "build");
        assert_eq!(steps[0].kind, "command");
    }

    #[tokio::test]
    async fn a_failing_step_names_the_backend_step_and_exit() {
        let provision = vec![step("fetch", "curl")];
        let last = Mutex::new(None);
        let error = run_steps(
            "test",
            "demo",
            &provision,
            &last,
            |_identity, _run| {
                Box::pin(async {
                    Ok(Execution {
                        exit_code: 7,
                        stdout: "the diagnostic".to_string(),
                        stderr: "a warning".to_string(),
                    })
                }) as BoxFuture<'_, Result<Execution, std::io::Error>>
            },
            |_step, error| Error::failed(error.to_string()),
        )
        .await
        .expect_err("the step failed");

        match error {
            Error::Provisioning { failure } => {
                assert_eq!(failure.backend.as_deref(), Some("test"));
                assert_eq!(failure.step.as_deref(), Some("fetch"));
                assert_eq!(failure.kind.as_deref(), Some("command"));
                assert_eq!(failure.exit_code, Some(7));
                assert!(!failure.remedy.is_empty());
                let output = failure.output.expect("output");
                assert!(output.contains("stdout:\nthe diagnostic"), "{output}");
                assert!(output.contains("stderr:\na warning"), "{output}");
            }
            other => panic!("expected a provisioning failure, got {other}"),
        }
    }

    #[tokio::test]
    async fn a_transport_error_is_mapped_by_the_backend() {
        let provision = vec![step("fetch", "curl")];
        let last = Mutex::new(None);
        let error = run_steps(
            "test",
            "demo",
            &provision,
            &last,
            |_identity, _run| {
                Box::pin(async { Err(std::io::Error::other("connection refused")) })
                    as BoxFuture<'_, Result<Execution, std::io::Error>>
            },
            |step, error| Error::failed(format!("{}: {error}", step.identity())),
        )
        .await
        .expect_err("the step failed");

        assert!(error.to_string().contains("fetch"), "{error}");
        assert!(error.to_string().contains("connection refused"), "{error}");
    }

    #[tokio::test]
    async fn the_budget_bounds_a_slow_sequence() {
        let provision = vec![step("slow", "sleep 60")];
        let last = Mutex::new(None);
        let sequence = run_steps(
            "test",
            "demo",
            &provision,
            &last,
            |_identity, _run| {
                Box::pin(async {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    Ok(Execution {
                        exit_code: 0,
                        stdout: String::new(),
                        stderr: String::new(),
                    })
                }) as BoxFuture<'_, Result<Execution, std::io::Error>>
            },
            |_step, error| Error::failed(error.to_string()),
        );

        let outcome = tokio::time::timeout(Duration::from_millis(20), sequence).await;
        assert!(outcome.is_err(), "the sequence should not finish");
        assert!(CREATE_BUDGET > Duration::from_secs(60));
    }

    #[test]
    fn output_is_bounded_at_the_boundary() {
        let exact = "a".repeat(ERROR_OUTPUT);
        assert_eq!(bounded_output(&exact), exact);
        let over = "a".repeat(ERROR_OUTPUT + 1);
        let bounded = bounded_output(&over);
        assert!(bounded.ends_with("[truncated]"), "{bounded}");
        let kept = bounded.trim_end_matches("\n[truncated]");
        assert_eq!(kept.chars().count(), ERROR_OUTPUT);
    }

    #[test]
    fn both_streams_are_labelled() {
        let output = combined_output("the diagnostic", "a warning");
        assert!(output.contains("stdout:\nthe diagnostic"), "{output}");
        assert!(output.contains("stderr:\na warning"), "{output}");
    }
}
