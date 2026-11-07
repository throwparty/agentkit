//! Preparing a host step: checks, environment capture and the store view.
//!
//! This is the only module that spawns a process. The orchestrator is its only
//! caller; the doctor uses [`check`], which never evaluates the shell hook.

pub mod checks;
pub mod closure;
pub mod devshell;
pub mod environment;
pub mod runner;

use std::path::{Path, PathBuf};

use crate::provision::kind;
use crate::sandbox::contract::{Contributions, ProvisionStep, SandboxAction, StoreView};

pub use checks::{Check, HostRequirements, check_host};
pub use closure::closure;
pub use runner::{CommandOutput, CommandRunner, SystemRunner};

/// Why a host step could not be prepared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrepareError {
    /// What is wrong.
    pub reason: String,
    /// What the user can do about it.
    pub remedy: String,
}

impl PrepareError {
    /// A preparation error with a reason and a remedy.
    pub fn new(reason: impl Into<String>, remedy: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            remedy: remedy.into(),
        }
    }
}

/// What a read-only host check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckReport {
    /// The host requirements.
    pub requirements: HostRequirements,
    /// The store view, or why it could not be derived.
    pub closure: Result<Vec<PathBuf>, PrepareError>,
    /// The view in force.
    pub store_view: StoreView,
    /// The installable the report is about, when the kind has one.
    pub installable: Option<String>,
}

/// What a step contributes when it is prepared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    /// The command to run inside the sandbox, when the step runs there.
    pub sandbox: Option<SandboxAction>,
    /// What the step contributes, when it runs on the host.
    pub contributions: Contributions,
}

/// Everything host preparation needs.
pub struct PrepareContext<'a> {
    /// The checkout the installable is relative to.
    pub project_root: &'a Path,
    /// The store view in force.
    pub store_view: StoreView,
    /// The process runner.
    pub runner: &'a dyn CommandRunner,
}

/// Everything a read-only host check needs.
pub struct CheckContext<'a> {
    /// The store view in force.
    pub store_view: StoreView,
    /// The process runner.
    pub runner: &'a dyn CommandRunner,
}

/// Prepare one planned step.
pub fn prepare(step: &ProvisionStep, ctx: &PrepareContext<'_>) -> Result<Prepared, PrepareError> {
    let kind =
        kind::validate(step).map_err(|error| PrepareError::new(error.reason, error.remedy))?;
    match kind.describe().name {
        "command" => {
            let sandbox = kind::command::action(&step.settings)
                .map_err(|error| PrepareError::new(error.reason, error.remedy))?;
            Ok(Prepared {
                sandbox: Some(sandbox),
                contributions: Contributions::default(),
            })
        }
        "devshell" => Ok(Prepared {
            sandbox: None,
            contributions: devshell::prepare(step, ctx)?,
        }),
        other => Err(PrepareError::new(
            format!("unknown provisioning kind '{other}'"),
            "use a kind this build supports",
        )),
    }
}

/// Check one planned step without evaluating the shell hook.
pub fn check_step(
    step: &ProvisionStep,
    ctx: &CheckContext<'_>,
) -> Result<CheckReport, PrepareError> {
    let kind =
        kind::validate(step).map_err(|error| PrepareError::new(error.reason, error.remedy))?;
    match kind.describe().name {
        "command" => Ok(CheckReport {
            requirements: HostRequirements {
                checks: Vec::new(),
                output_path: None,
                baseline: None,
            },
            closure: Ok(Vec::new()),
            store_view: ctx.store_view,
            installable: None,
        }),
        "devshell" => devshell::check(step, ctx),
        other => Err(PrepareError::new(
            format!("unknown provisioning kind '{other}'"),
            "use a kind this build supports",
        )),
    }
}

/// Check the host and the store view without evaluating the shell hook.
pub fn check(installable: &str, view: StoreView, runner: &dyn CommandRunner) -> CheckReport {
    let requirements = checks::check_host(installable, runner);
    let closure = if requirements.ok() {
        match &requirements.output_path {
            Some(path) => closure::closure(path, view, runner),
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
    CheckReport {
        requirements,
        closure,
        store_view: view,
        installable: Some(installable.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::path::Path;
    use std::sync::Mutex;

    use super::runner::CommandOutput;
    use super::*;

    const JSON: &str = r#"{"variables":{"out":{"value":"/nix/store/abc-devshell"}}}"#;

    struct Recorded {
        outputs: Mutex<VecDeque<CommandOutput>>,
    }

    impl Recorded {
        fn new(outputs: Vec<CommandOutput>) -> Self {
            Self {
                outputs: Mutex::new(outputs.into()),
            }
        }
    }

    impl CommandRunner for Recorded {
        fn run(
            &self,
            _program: &str,
            _args: &[String],
            _env: Option<&BTreeMap<String, String>>,
            _cwd: Option<&Path>,
        ) -> Result<CommandOutput, String> {
            Ok(self
                .outputs
                .lock()
                .expect("lock")
                .pop_front()
                .expect("a recorded output"))
        }
    }

    fn ok(stdout: &str) -> CommandOutput {
        CommandOutput {
            status: 0,
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    fn ok_with_stderr(stdout: &str, stderr: &str) -> CommandOutput {
        CommandOutput {
            status: 0,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    fn fail(stderr: &str) -> CommandOutput {
        CommandOutput {
            status: 1,
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    fn devshell_step(installable: &str) -> ProvisionStep {
        ProvisionStep {
            identity: "dev".to_string(),
            kind: "devshell".to_string(),
            settings: [(
                "installable".to_string(),
                toml::Value::String(installable.to_string()),
            )]
            .into(),
        }
    }

    #[test]
    fn a_devshell_check_reports_requirements_and_closure() {
        let step = devshell_step("./nix");
        let ctx = CheckContext {
            store_view: StoreView::Closure,
            runner: &Recorded::new(vec![
                ok("nix 2.34.9"),
                ok(JSON),
                ok("/nix/store/abc-devshell"),
                ok("/nix/store/a\n/nix/store/b"),
            ]),
        };
        let report = check_step(&step, &ctx).expect("check");
        assert_eq!(report.installable.as_deref(), Some("./nix"));
        assert_eq!(report.closure.expect("closure").len(), 2);
    }

    #[test]
    fn a_command_step_prepares_a_sandbox_action() {
        let step = ProvisionStep {
            identity: "fetch".to_string(),
            kind: "command".to_string(),
            settings: [("run".to_string(), toml::Value::String("true".to_string()))].into(),
        };
        let ctx = PrepareContext {
            project_root: Path::new("/repo"),
            store_view: StoreView::Closure,
            runner: &Recorded::new(Vec::new()),
        };
        let prepared = prepare(&step, &ctx).expect("prepare");
        assert_eq!(prepared.sandbox.expect("action").run, "true");
        assert!(prepared.contributions.environment.is_empty());
    }

    #[test]
    fn a_missing_nix_is_reported() {
        let report = check(
            "./nix",
            StoreView::Closure,
            &Recorded::new(vec![fail("nix: command not found")]),
        );
        assert!(!report.requirements.ok());
        assert_eq!(report.requirements.checks[0].name, "nix");
        assert!(report.requirements.checks[0].remedy.is_some());
    }

    #[test]
    fn disabled_flakes_are_named() {
        let report = check(
            "./nix",
            StoreView::Closure,
            &Recorded::new(vec![
                ok("nix 2.34.9"),
                fail("error: experimental Nix feature 'flakes' is disabled"),
            ]),
        );
        assert!(!report.requirements.ok());
        assert_eq!(report.requirements.checks[1].name, "flakes enabled");
        assert!(
            report.requirements.checks[1]
                .remedy
                .as_deref()
                .is_some_and(|remedy| remedy.contains("flakes"))
        );
    }

    #[test]
    fn an_invalid_devshell_is_refused() {
        let report = check(
            "./nix",
            StoreView::Closure,
            &Recorded::new(vec![ok("nix 2.34.9"), ok(JSON), fail("path is not valid")]),
        );
        assert!(!report.requirements.ok());
        assert_eq!(
            report.requirements.checks[2].name,
            "devShell is valid in the store"
        );
        assert!(report.closure.is_err());
    }

    #[test]
    fn the_closure_is_sorted_and_deduplicated() {
        let report = check(
            "./nix",
            StoreView::Closure,
            &Recorded::new(vec![
                ok("nix 2.34.9"),
                ok(JSON),
                ok("/nix/store/abc-devshell"),
                ok("/nix/store/b\n/nix/store/a\n/nix/store/a\n"),
            ]),
        );
        let closure = report.closure.expect("closure");
        assert_eq!(
            closure,
            vec![PathBuf::from("/nix/store/a"), PathBuf::from("/nix/store/b")]
        );
    }

    #[test]
    fn the_whole_view_is_one_mount() {
        let report = check(
            "./nix",
            StoreView::Whole,
            &Recorded::new(vec![
                ok("nix 2.34.9"),
                ok(JSON),
                ok("/nix/store/abc-devshell"),
            ]),
        );
        assert_eq!(
            report.closure.expect("closure"),
            vec![PathBuf::from("/nix/store")]
        );
    }

    #[test]
    fn a_dirty_tree_is_reported() {
        let report = check(
            "./nix",
            StoreView::Closure,
            &Recorded::new(vec![
                ok("nix 2.34.9"),
                ok_with_stderr(JSON, "warning: Git tree is dirty"),
                ok("/nix/store/abc-devshell"),
                ok("/nix/store/a"),
            ]),
        );
        assert!(
            report
                .requirements
                .checks
                .iter()
                .any(|check| check.name == "dirty tree"),
            "{:?}",
            report.requirements.checks
        );
    }

    #[test]
    #[ignore = "requires nix and a nixpkgs input; run with --ignored"]
    fn an_unrealised_devshell_is_refused_without_building_it() {
        let marker = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let directory = tempfile::tempdir().expect("tempdir");
        let system = nix_system();
        std::fs::write(
            directory.path().join("flake.nix"),
            format!(
                r#"{{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/release-26.05";
  outputs = {{ self, nixpkgs }}:
    let
      system = "{system}";
      pkgs = import nixpkgs {{ inherit system; }};
    in {{
      devShells.${{system}}.default = pkgs.mkShell {{
        shellHook = "echo {marker}";
      }};
    }};
}}
"#
            ),
        )
        .expect("write the flake");

        let installable = directory.path().to_string_lossy().to_string();
        let requirements = checks::check_installable(&installable, &SystemRunner::default());
        assert!(!requirements.ok(), "{:?}", requirements.checks);
        assert!(
            requirements.baseline.is_none(),
            "print-dev-env must not run for an unrealised devShell"
        );
        assert!(
            requirements
                .checks
                .iter()
                .any(|check| check.name == "devShell is valid in the store" && !check.ok),
            "{:?}",
            requirements.checks
        );
    }

    /// The Nix system name for the platform the tests run on.
    fn nix_system() -> String {
        let os = match std::env::consts::OS {
            "macos" => "darwin",
            other => other,
        };
        format!("{}-{os}", std::env::consts::ARCH)
    }
}
