use std::process::ExitCode;

use crate::backends;
use crate::config::Config;
use crate::config_loader;
use crate::mcp;
use crate::orchestrator::{
    DeleteGuards, HostReport, InspectionReport, PlannedStep, Preflight, RestoreFlags, Sandboxes,
    StagedMerge, preflight_host,
};
use crate::sandbox::contract::{
    ContributionSupport, Error, ProvisioningClass, SandboxStatus, StoreView,
};
use crate::sandbox::report::{Readiness, State};
use clap::{CommandFactory, Parser, Subcommand};

#[derive(Parser)]
#[command(
    author,
    version,
    about = "Litterbox - sandboxing tool for software engineering agents",
    long_about = "Litterbox provides isolated sandbox environments combining SCM branches and confined compute.\n\
                  Each sandbox pairs a Git branch with a sandboxing mechanism for safe, isolated development work."
)]
struct Cli {
    /// Select the backend for this invocation instead of the configured one.
    #[arg(long, global = true)]
    backend: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List all sandboxes in the current repository
    List,

    /// Run the MCP (Model Control Protocol) server over stdio
    Stdio,

    /// Pause one or more sandboxes
    Pause {
        /// Name of the sandbox to pause
        #[arg(
            required_unless_present_any = ["all", "machine"],
            help = "Sandbox name to pause"
        )]
        name: Option<String>,

        /// Pause all sandboxes in the current repository
        #[arg(
            long,
            alias = "all-envs",
            conflicts_with = "machine",
            help = "Pause all sandboxes in this repository"
        )]
        all: bool,

        /// Pause all Litterbox sandboxes across all repositories
        #[arg(
            long,
            alias = "all-repos",
            conflicts_with = "all",
            help = "Pause all Litterbox sandboxes on this machine"
        )]
        machine: bool,
    },

    /// Resume a paused sandbox
    Resume {
        /// Name of the sandbox to resume
        name: String,
    },

    /// Delete a sandbox
    Delete {
        /// Name of the sandbox to delete
        name: String,

        /// Delete even if the sandbox's compute is running.
        #[arg(long)]
        ignore_running: bool,

        /// Delete even if the record or its compute cannot be read.
        #[arg(long)]
        ignore_invalid: bool,

        /// Delete even if the sandbox branch, resolution or staging workspace
        /// may hold work the shared branch does not.
        #[arg(long)]
        ignore_unmerged: bool,
    },

    /// Report the backends available on this machine, their readiness and their
    /// capabilities.
    Backends {
        /// Emit structured JSON instead of a table.
        #[arg(long)]
        json: bool,
    },

    /// Report whether this project can create a sandbox on this machine.
    Doctor {
        /// Emit structured JSON instead of a table.
        #[arg(long)]
        json: bool,
    },

    /// Add the Git remote that holds this project's sandbox branches
    Remote {
        /// Name for the remote
        #[arg(default_value = "litterbox")]
        name: String,
    },

    /// Execute a command in a sandbox
    Shell {
        /// Name of the sandbox to run the command in
        name: String,

        /// Command and arguments to execute
        #[arg(required = true, trailing_var_arg = true)]
        command: Vec<String>,
    },

    /// Report how a sandbox's branch and workspace diverged
    Status {
        /// Name of the sandbox to report
        name: String,

        /// Emit structured JSON instead of a table.
        #[arg(long)]
        json: bool,
    },

    /// List the commits a sandbox's shared branch pointed at
    History {
        /// Name of the sandbox to list
        name: String,

        /// Emit structured JSON instead of a table.
        #[arg(long)]
        json: bool,
    },

    /// Restore a sandbox to a listed step by adding a forward commit
    Restore {
        /// Name of the sandbox to restore
        name: String,

        /// Step number from `history`, or a commit ID it lists
        step: String,

        /// Discard a held merge, unsaved work or a staging workspace
        #[arg(long)]
        discard_work: bool,

        /// Discard an open resolution
        #[arg(long)]
        discard_resolution: bool,

        /// Emit structured JSON instead of a table.
        #[arg(long)]
        json: bool,
    },

    /// Stage or finish a merge with a rewritten sandbox branch
    Reconcile {
        /// Name of the sandbox to reconcile
        name: String,

        /// Land the merge commit on the branch
        #[arg(long, conflicts_with = "abort")]
        finish: bool,

        /// Remove the staging worktree and leave the branch
        #[arg(long, conflicts_with = "finish")]
        abort: bool,

        /// Emit structured JSON instead of a table.
        #[arg(long)]
        json: bool,
    },

    /// Generate reference documentation
    #[command(hide = true)]
    Docgen {
        #[command(subcommand)]
        kind: DocgenCommand,
    },
}

#[derive(Subcommand)]
enum DocgenCommand {
    /// Generate CLI reference documentation
    Cli,

    /// Generate MCP tools reference documentation
    Mcp,
}

pub async fn run() -> ExitCode {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "agentkit_litterbox=info".into()),
        )
        .with_writer(std::io::stderr)
        .try_init();

    let cli = Cli::parse();
    let backend = cli.backend;
    match cli.command {
        Commands::List => handle_list(backend).await,
        Commands::Stdio => handle_stdio(backend).await,
        Commands::Pause { name, all, machine } => handle_pause(backend, name, all, machine).await,
        Commands::Resume { name } => handle_resume(backend, name).await,
        Commands::Delete {
            name,
            ignore_running,
            ignore_invalid,
            ignore_unmerged,
        } => {
            handle_delete(
                backend,
                name,
                DeleteGuards {
                    running: ignore_running,
                    invalid: ignore_invalid,
                    unmerged: ignore_unmerged,
                },
            )
            .await
        }
        Commands::Remote { name } => handle_remote(backend, name),
        Commands::Shell { name, command } => handle_shell(backend, name, command).await,
        Commands::Status { name, json } => handle_status(backend, name, json).await,
        Commands::History { name, json } => handle_history(backend, name, json).await,
        Commands::Restore {
            name,
            step,
            discard_work,
            discard_resolution,
            json,
        } => handle_restore(backend, name, step, discard_work, discard_resolution, json).await,
        Commands::Reconcile {
            name,
            finish,
            abort,
            json,
        } => handle_reconcile(backend, name, finish, abort, json).await,
        Commands::Backends { json } => handle_backends(backend, json).await,
        Commands::Doctor { json } => handle_doctor(backend, json).await,
        Commands::Docgen { kind } => handle_docgen(kind),
    }
}

fn load_config(backend: Option<String>) -> Result<Config, Error> {
    let config = load_config_quiet(backend)?;
    for notice in &config.notices {
        eprintln!("configuration: {notice}");
    }
    Ok(config)
}

/// Load the merged configuration without printing its notices.
fn load_config_quiet(backend: Option<String>) -> Result<Config, Error> {
    let mut config = config_loader::load_final().map_err(|error| Error::Misconfigured {
        message: error.to_string(),
    })?;
    if let Some(backend) = backend {
        config.backend = Some(backend);
    }
    Ok(config)
}

/// Load only the user's file, for commands that are host-scoped.
fn load_host_config(backend: Option<String>) -> Result<Config, Error> {
    let mut config = config_loader::load_local_only().map_err(|error| Error::Misconfigured {
        message: error.to_string(),
    })?;
    if let Some(backend) = backend {
        config.backend = Some(backend);
    }
    for notice in &config.notices {
        eprintln!("configuration: {notice}");
    }
    Ok(config)
}

fn project(config: Config) -> Result<Sandboxes, Error> {
    crate::compose::open(config).map_err(Error::from)
}

async fn handle_stdio(backend: Option<String>) -> ExitCode {
    if let Err(error) = mcp::run_stdio(backend).await {
        return report(Error::failed(error.to_string()));
    }
    ExitCode::from(0)
}

fn handle_remote(backend: Option<String>, name: String) -> ExitCode {
    let config = match load_config(backend) {
        Ok(config) => config,
        Err(error) => return report(error),
    };
    let sandboxes = match crate::compose::open_existing(config).map_err(Error::from) {
        Ok(sandboxes) => sandboxes,
        Err(error) => return report(error),
    };
    let (store, added) = match sandboxes.expose_remote(&name) {
        Ok(result) => result,
        Err(error) => return report(error),
    };
    if added {
        println!("added remote '{name}' -> '{}'", store.display());
    } else {
        println!("remote '{name}' already points at '{}'", store.display());
    }
    println!(
        "git fetch {name} '+refs/heads/litterbox/*:refs/remotes/{name}/*' '+refs/litterbox/workspaces/*:refs/remotes/{name}/workspaces/*'"
    );
    ExitCode::from(0)
}

async fn handle_status(backend: Option<String>, name: String, json: bool) -> ExitCode {
    let config = match load_config(backend) {
        Ok(config) => config,
        Err(error) => return report(error),
    };
    let sandboxes = match project(config) {
        Ok(sandboxes) => sandboxes,
        Err(error) => return report(error),
    };
    let divergence = match sandboxes.report(&name).await {
        Ok(report) => report,
        Err(error) => return report(error),
    };

    if json {
        match serde_json::to_string_pretty(&divergence) {
            Ok(text) => println!("{text}"),
            Err(error) => return report(Error::failed(error.to_string())),
        }
        return ExitCode::from(0);
    }

    println!("sandbox   {name}");
    println!("base      {}", divergence.base);
    println!("branch    {}", divergence.branch);
    match &divergence.sandbox {
        Some(sandbox) => println!("sandbox   {sandbox}"),
        None => println!("sandbox   absent"),
    }
    if let Some(next) = &divergence.next {
        println!("next      {next}");
    }
    if divergence.conflicts.is_empty() {
        println!("conflicts none");
    } else {
        let paths: Vec<String> = divergence
            .conflicts
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        println!("conflicts {}", paths.join(", "));
    }
    if let Some(path) = &divergence.worktree {
        println!("worktree  {}", path.display());
    }
    if let Some(reflog) = &divergence.reflog {
        println!("reflog    {reflog}");
        println!("previous  git reflog show {reflog}");
    }
    ExitCode::from(0)
}

async fn handle_history(backend: Option<String>, name: String, json: bool) -> ExitCode {
    let config = match load_config(backend) {
        Ok(config) => config,
        Err(error) => return report(error),
    };
    let sandboxes = match project(config) {
        Ok(sandboxes) => sandboxes,
        Err(error) => return report(error),
    };
    let history = match sandboxes.history(&name).await {
        Ok(history) => history,
        Err(error) => return report(error),
    };

    if json {
        let steps: Vec<serde_json::Value> = history
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                serde_json::json!({
                    "step": index + 1,
                    "revision": step.revision.as_str(),
                    "time": step.time,
                    "time_utc": utc_time(step.time),
                    "description": step.description,
                    "synced": step.synced,
                })
            })
            .collect();
        let value = serde_json::json!({
            "sandbox": history.sandbox,
            "state": history.state,
            "sandbox_tip": history.sandbox_tip,
            "next": history.next,
            "steps": steps,
        });
        match serde_json::to_string_pretty(&value) {
            Ok(text) => println!("{text}"),
            Err(error) => return report(Error::failed(error.to_string())),
        }
        return ExitCode::from(0);
    }

    println!("sandbox  {}", history.sandbox);
    println!("state    {}", history.state);
    if let Some(tip) = &history.sandbox_tip {
        println!("sandbox  {tip}");
    }
    if history.steps.is_empty() {
        println!("history  none recorded");
    } else {
        println!();
        println!("{:<5}{:<14}{:<21}DESCRIPTION", "STEP", "COMMIT", "TIME");
        for (index, step) in history.steps.iter().enumerate() {
            let short: String = step.revision.as_str().chars().take(12).collect();
            let description = if step.synced {
                format!("{} (synced)", step.description)
            } else {
                step.description.clone()
            };
            println!(
                "{:<5}{:<14}{:<21}{}",
                index + 1,
                short,
                utc_time(step.time),
                description
            );
        }
    }
    if let Some(next) = &history.next {
        println!("\n{next}");
    }
    ExitCode::from(0)
}

async fn handle_restore(
    backend: Option<String>,
    name: String,
    step: String,
    discard_work: bool,
    discard_resolution: bool,
    json: bool,
) -> ExitCode {
    let config = match load_config(backend) {
        Ok(config) => config,
        Err(error) => return report(error),
    };
    let sandboxes = match project(config) {
        Ok(sandboxes) => sandboxes,
        Err(error) => return report(error),
    };
    let result = match sandboxes
        .restore(
            &name,
            &step,
            RestoreFlags {
                discard_work,
                discard_resolution,
            },
        )
        .await
    {
        Ok(result) => result,
        Err(error) => return report(error),
    };

    if json {
        let value = serde_json::json!({
            "sandbox": result.sandbox,
            "target": result.target,
            "revision": result.revision,
            "created": result.created,
        });
        match serde_json::to_string_pretty(&value) {
            Ok(text) => println!("{text}"),
            Err(error) => return report(Error::failed(error.to_string())),
        }
        return ExitCode::from(0);
    }

    println!("sandbox  {}", result.sandbox);
    println!("target   {}", result.target);
    if result.created {
        println!("commit   {}", result.revision);
    } else {
        println!("commit   none (the tree already matched)");
        println!("head     {}", result.revision);
    }
    ExitCode::from(0)
}

/// Format Unix seconds as UTC, without adding a date library.
fn utc_time(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let seconds = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z",
        seconds / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60
    )
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month as u32, day as u32)
}

async fn handle_reconcile(
    backend: Option<String>,
    name: String,
    finish: bool,
    abort: bool,
    json: bool,
) -> ExitCode {
    let config = match load_config(backend) {
        Ok(config) => config,
        Err(error) => return report(error),
    };
    let sandboxes = match project(config) {
        Ok(sandboxes) => sandboxes,
        Err(error) => return report(error),
    };

    if abort {
        return match sandboxes.abort_stage(&name).await {
            Ok(()) => {
                println!("removed the staging worktree for '{name}'");
                ExitCode::from(0)
            }
            Err(error) => report(error),
        };
    }

    if finish {
        return match sandboxes.finish(&name).await {
            Ok(StagedMerge::Landed { branch }) => {
                if json {
                    match serde_json::to_string_pretty(&serde_json::json!({
                        "landed": true,
                        "branch": branch,
                    })) {
                        Ok(text) => println!("{text}"),
                        Err(error) => return report(Error::failed(error.to_string())),
                    }
                } else {
                    println!("the branch is now {branch}");
                }
                ExitCode::from(0)
            }
            Ok(StagedMerge::Deferred { command }) => {
                println!("run this in your checkout, then run --finish again: {command}");
                ExitCode::from(0)
            }
            Err(error) => report(error),
        };
    }

    match sandboxes.stage(&name).await {
        Ok(staged) => {
            if json {
                match serde_json::to_string_pretty(&staged) {
                    Ok(text) => println!("{text}"),
                    Err(error) => return report(Error::failed(error.to_string())),
                }
            } else {
                println!("worktree  {}", staged.path.display());
                for path in &staged.conflicts {
                    println!("conflict  {}", path.display());
                }
            }
            ExitCode::from(0)
        }
        Err(error) => report(error),
    }
}

async fn handle_list(backend: Option<String>) -> ExitCode {
    let config = match load_config(backend) {
        Ok(config) => config,
        Err(error) => return report(error),
    };
    let sandboxes = match project(config) {
        Ok(sandboxes) => sandboxes,
        Err(error) => return report(error),
    };

    let listing = match sandboxes.list() {
        Ok(listing) => listing,
        Err(error) => return report(error),
    };

    println!("records {}", sandboxes.records_dir().display());

    let mut failed = false;
    for entry in &listing.corrupt {
        println!("{entry}");
        failed = true;
    }
    if let Some(error) = &listing.branch_error {
        println!("branches: {error}");
        failed = true;
    }
    for name in &listing.orphans {
        println!(
            "{name} orphan; the branch has no record, so the compute it was paired with \
             cannot be addressed by Litterbox"
        );
    }
    let sources = listing.sources.clone();
    let inspections = match sandboxes.inspect_listing(listing).await {
        Ok(inspections) => inspections,
        Err(error) => return report(error),
    };
    for (name, report) in inspections {
        let source = sources.get(&name).map(String::as_str).unwrap_or("clean");
        let suffix = if source == "clean" {
            String::new()
        } else {
            format!(" {source}")
        };
        match report {
            InspectionReport::Inspected(inspection) => {
                println!(
                    "{} {}{} {}",
                    name,
                    status_label(&inspection.status),
                    suffix,
                    inspection.backend
                );
                if listing_failed(&inspection.status) {
                    failed = true;
                }
            }
            InspectionReport::Status { status, reason, .. } => {
                match &status {
                    SandboxStatus::Unknown => println!("{name} unknown{suffix} {reason}"),
                    _ => println!("{name} {}{suffix}", status_label(&status)),
                }
                if listing_failed(&status) {
                    failed = true;
                }
            }
        }
    }

    ExitCode::from(if failed { 1 } else { 0 })
}

async fn handle_pause(
    backend: Option<String>,
    name: Option<String>,
    all: bool,
    machine: bool,
) -> ExitCode {
    if machine {
        let config = match load_host_config(backend) {
            Ok(config) => config,
            Err(error) => return report(error),
        };
        return match crate::orchestrator::machine::pause_machine(&config).await {
            Ok(summary) => pause_exit(summary),
            Err(error) => report(error),
        };
    }

    let config = match load_config(backend) {
        Ok(config) => config,
        Err(error) => return report(error),
    };

    let sandboxes = match project(config) {
        Ok(sandboxes) => sandboxes,
        Err(error) => return report(error),
    };

    if all {
        return match sandboxes.pause_listed().await {
            Ok(summary) => pause_exit(summary),
            Err(error) => report(error),
        };
    }

    let Some(name) = name else {
        return report(Error::failed("a sandbox name is required"));
    };

    match sandboxes.pause(&name).await {
        Ok(()) => {
            println!("Paused {name}");
            ExitCode::from(0)
        }
        Err(error) => report(error),
    }
}

fn pause_exit(summary: crate::orchestrator::PauseSummary) -> ExitCode {
    println!(
        "Paused {} sandbox(es), skipped {}",
        summary.paused, summary.skipped
    );
    for failure in &summary.failures {
        eprintln!("failed: {failure}");
    }
    ExitCode::from(if summary.is_complete() { 0 } else { 1 })
}

async fn handle_resume(backend: Option<String>, name: String) -> ExitCode {
    let config = match load_config(backend) {
        Ok(config) => config,
        Err(error) => return report(error),
    };
    let sandboxes = match project(config) {
        Ok(sandboxes) => sandboxes,
        Err(error) => return report(error),
    };

    match sandboxes.resume(&name).await {
        Ok(()) => {
            println!("Resumed {name}");
            ExitCode::from(0)
        }
        Err(error) => report(error),
    }
}

async fn handle_delete(backend: Option<String>, name: String, guards: DeleteGuards) -> ExitCode {
    let config = match load_config(backend) {
        Ok(config) => config,
        Err(error) => return report(error),
    };
    let sandboxes = match project(config) {
        Ok(sandboxes) => sandboxes,
        Err(error) => return report(error),
    };

    match sandboxes.delete(&name, guards).await {
        Ok(deletion) => {
            print!("Deleted {name}");
            if let Some(path) = &deletion.record_path {
                print!("; removed record {}", path.display());
            }
            if !deletion.state_paths.is_empty() {
                print!(
                    "; removed state {}",
                    deletion
                        .state_paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            if !deletion.branches.is_empty() {
                print!("; removed {}", deletion.branches.join(", "));
            }
            if let Some(staging) = &deletion.staging {
                print!("; removed staging {}", staging.display());
            }
            if !deletion.unused.is_empty() {
                print!("; unused flags {}", deletion.unused.join(", "));
            }
            if !deletion.compute_stopped {
                print!("; compute could not be stopped");
            }
            if let Some(detail) = &deletion.detail {
                print!(" ({detail})");
            }
            println!();
            ExitCode::from(0)
        }
        Err(error) => report(error),
    }
}

async fn handle_shell(backend: Option<String>, name: String, command: Vec<String>) -> ExitCode {
    let config = match load_config(backend) {
        Ok(config) => config,
        Err(error) => return report(error),
    };
    let sandboxes = match project(config) {
        Ok(sandboxes) => sandboxes,
        Err(error) => return report(error),
    };

    let performed = match sandboxes.exec(&name, &command).await {
        Ok(result) => result,
        Err(error) => return report(error),
    };
    if let Some(recovery) = &performed.recovery {
        eprintln!(
            "warning: sandbox '{}' was recreated from '{}' on branch '{}' (source: {})",
            recovery.sandbox, recovery.action, recovery.branch, recovery.source
        );
        if let Some(fallback) = &recovery.fallback {
            eprintln!("warning: {fallback}");
        }
    }
    let result = performed.value;

    if !result.stdout.is_empty() {
        print!("{}", result.stdout);
    }
    if !result.stderr.is_empty() {
        eprint!("{}", result.stderr);
    }
    if result.exit_code != 0 {
        eprintln!("command failed: {result}");
    }

    ExitCode::from(shell_exit_code(result.exit_code))
}

/// Namespaced: the taxonomy owns 0 to 7, so a sandboxed command's own status
/// can never be mistaken for one of its classes.
fn shell_exit_code(exit_code: i32) -> u8 {
    if exit_code == 0 {
        0
    } else {
        let code = u8::try_from(exit_code.clamp(1, 127)).unwrap_or(1);
        128 + code
    }
}

async fn handle_backends(backend: Option<String>, json: bool) -> ExitCode {
    let config = match load_host_config(backend) {
        Ok(config) => config,
        Err(error) => return report(error),
    };

    let reports = backends::introspect(&config).await;

    match render_backends(&reports, json) {
        Ok(text) => {
            println!("{text}");
            ExitCode::from(0)
        }
        Err(error) => report(Error::Misconfigured {
            message: error.to_string(),
        }),
    }
}

/// One step's state in the doctor's report, distinct from host readiness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum DoctorState {
    Available,
    Degraded,
    Unavailable,
    Denied,
    Failed,
}

/// A provisioning failure, with the class its message cannot carry.
#[derive(serde::Serialize)]
struct DoctorError {
    class: String,
    message: String,
}

/// A structured answer to whether this project can create a sandbox here.
#[derive(serde::Serialize)]
struct DoctorReport {
    backend: String,
    project_slug: String,
    readiness: Readiness,
    capabilities: Vec<crate::sandbox::report::Capability>,
    supported_backends: Option<Vec<String>>,
    notices: Vec<String>,
    store_view: StoreView,
    support: Vec<ContributionSupport>,
    steps: Vec<DoctorStep>,
    error: Option<DoctorError>,
}

/// One host requirement in the doctor's report.
#[derive(Clone, Debug, serde::Serialize)]
struct DoctorRequirement {
    name: String,
    ok: bool,
    detail: String,
    remedy: Option<String>,
}

/// One provisioning step in the doctor's report.
#[derive(Debug, serde::Serialize)]
struct DoctorStep {
    identity: Option<String>,
    kind: Option<String>,
    state: DoctorState,
    class: Option<ProvisioningClass>,
    reason: Option<String>,
    remedy: Option<String>,
    installable: Option<String>,
    closure_count: Option<usize>,
    requirements: Vec<DoctorRequirement>,
}

pub async fn handle_doctor(backend: Option<String>, json: bool) -> ExitCode {
    let requested_backend = backend.clone();
    let config = match load_config_quiet(backend) {
        Ok(config) => config,
        Err(error) => return report_doctor_failure(error, json, requested_backend.as_deref()),
    };
    let project_slug = match crate::orchestrator::resolve_project_slug(&config) {
        Ok(slug) => slug,
        Err(error) => {
            return report_doctor_failure(
                Error::Misconfigured {
                    message: error.to_string(),
                },
                json,
                config.backend.as_deref(),
            );
        }
    };
    let selected = match backends::build(&config) {
        Ok(selected) => selected,
        Err(error) => return report_doctor_failure(error, json, config.backend.as_deref()),
    };
    let selected_name = selected.name().to_string();
    let (readiness, capabilities, support) = match backends::describe(&selected).await {
        Ok(described) => described,
        Err(error) => return report_doctor_failure(error, json, Some(&selected_name)),
    };
    let (plan, host_reports) =
        preflight_host(&config, &project_slug, selected.name(), &support).await;
    let (doctor_report, failure) = build_doctor_report(
        &config,
        &project_slug,
        selected.name(),
        readiness,
        capabilities,
        &support,
        Some(plan),
        host_reports,
    );

    tracing::info!(
        backend = %doctor_report.backend,
        steps = doctor_report.steps.len(),
        error = doctor_report
            .error
            .as_ref()
            .map(|error| error.class.as_str())
            .unwrap_or(""),
        "doctor report"
    );
    match render_doctor(&doctor_report, json) {
        Ok(text) => {
            println!("{text}");
            ExitCode::from(doctor_exit_code(
                &doctor_report,
                failure,
                config.accept_degraded,
            ))
        }
        Err(render_error) => report(render_error),
    }
}

/// Build the preflight report from the facts the backend answered.
///
/// The builder is pure so the state mapping and the exit code can be tested
/// without a CLI, a daemon or a mutation.
#[allow(clippy::too_many_arguments)]
fn build_doctor_report(
    config: &Config,
    project_slug: &str,
    backend: &str,
    readiness: Readiness,
    capabilities: Vec<crate::sandbox::report::Capability>,
    support: &[ContributionSupport],
    plan: Option<Preflight>,
    host_reports: Vec<HostReport>,
) -> (DoctorReport, Option<agentkit_common::exit::ExitClass>) {
    let mut steps = Vec::new();
    let mut error = None;
    let mut failure = None;

    let plan = plan
        .unwrap_or_else(|| crate::orchestrator::preflight(config, project_slug, backend, support));
    if let Some(resolve_error) = plan.failure {
        failure = Some(resolve_error.exit_class());
        error = Some(DoctorError {
            class: resolve_error.class().as_str().to_string(),
            message: resolve_error.to_string(),
        });
    }
    for planned in plan.steps {
        match planned {
            PlannedStep::Resolved {
                step,
                support: step_support,
                refusal,
                ..
            } => {
                let host = host_reports
                    .iter()
                    .find(|report| report.identity == step.identity);
                let (state, class, reason, remedy) = if let Some(refusal) = refusal {
                    (
                        DoctorState::Denied,
                        Some(refusal.class),
                        Some(refusal.reason),
                        Some(refusal.remedy),
                    )
                } else if let Some(failure) = host.and_then(|report| report.failure.as_ref()) {
                    (
                        doctor_state(failure.class),
                        Some(failure.class),
                        Some(failure.reason.clone()),
                        Some(failure.remedy.clone()),
                    )
                } else {
                    contribution_state(&step_support)
                };
                steps.push(DoctorStep {
                    identity: Some(step.identity.clone()),
                    kind: Some(step.kind.clone()),
                    state,
                    class,
                    reason,
                    remedy,
                    installable: host.and_then(|report| report.installable.clone()),
                    closure_count: host.and_then(|report| report.closure_count),
                    requirements: host
                        .map(|report| {
                            report
                                .requirements
                                .iter()
                                .map(|requirement| DoctorRequirement {
                                    name: requirement.name.clone(),
                                    ok: requirement.ok,
                                    detail: requirement.detail.clone(),
                                    remedy: requirement.remedy.clone(),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                });
            }
            PlannedStep::Failed {
                failure: step_failure,
            } => {
                let step_error = Error::Provisioning {
                    failure: Box::new(step_failure.clone()),
                };
                if failure.is_none() {
                    failure = Some(step_error.exit_class());
                    error = Some(DoctorError {
                        class: step_error.class().as_str().to_string(),
                        message: step_error.to_string(),
                    });
                }
                steps.push(DoctorStep {
                    identity: step_failure.step.clone(),
                    kind: step_failure.kind.clone(),
                    state: DoctorState::Unavailable,
                    class: Some(step_failure.class),
                    reason: Some(step_failure.reason),
                    remedy: Some(step_failure.remedy).filter(|remedy| !remedy.is_empty()),
                    installable: None,
                    closure_count: None,
                    requirements: Vec::new(),
                });
            }
        }
    }

    (
        DoctorReport {
            backend: backend.to_string(),
            project_slug: project_slug.to_string(),
            readiness,
            capabilities,
            supported_backends: config.project.backends.clone(),
            notices: config.notices.clone(),
            store_view: config.store_view,
            support: support.to_vec(),
            steps,
            error,
        },
        failure,
    )
}

fn doctor_state(class: ProvisioningClass) -> DoctorState {
    match class {
        ProvisioningClass::Failed => DoctorState::Failed,
        ProvisioningClass::Denied => DoctorState::Denied,
        _ => DoctorState::Unavailable,
    }
}

fn contribution_state(
    support: &[ContributionSupport],
) -> (
    DoctorState,
    Option<ProvisioningClass>,
    Option<String>,
    Option<String>,
) {
    if let Some(entry) = support
        .iter()
        .find(|entry| entry.state == State::Unavailable)
    {
        return (
            DoctorState::Unavailable,
            None,
            entry.reason.clone(),
            entry.remedy.clone(),
        );
    }
    if let Some(entry) = support.iter().find(|entry| entry.state == State::Degraded) {
        return (
            DoctorState::Degraded,
            None,
            entry.reason.clone(),
            entry.remedy.clone(),
        );
    }
    (DoctorState::Available, None, None, None)
}

/// The process exit code the doctor reports.
fn doctor_exit_code(
    report: &DoctorReport,
    failure: Option<agentkit_common::exit::ExitClass>,
    accept_degraded: bool,
) -> u8 {
    let denied = report
        .steps
        .iter()
        .any(|step| step.state == DoctorState::Denied);
    let steps_ready = report
        .steps
        .iter()
        .all(|step| matches!(step.state, DoctorState::Available | DoctorState::Degraded));
    let ready = report.error.is_none()
        && backends::readiness_is_usable(&report.readiness, accept_degraded)
        && steps_ready;
    if let Some(failure) = failure {
        failure.code()
    } else if !backends::readiness_is_usable(&report.readiness, accept_degraded) {
        agentkit_common::exit::ExitClass::Unavailable.code()
    } else if denied {
        agentkit_common::exit::ExitClass::Denied.code()
    } else if ready {
        0
    } else {
        1
    }
}

/// Render the doctor report as a table or as JSON, so a test can read it.
fn render_doctor(report: &DoctorReport, json: bool) -> Result<String, Error> {
    if json {
        return serde_json::to_string_pretty(report)
            .map_err(|error| Error::failed(error.to_string()));
    }

    let mut lines = vec![
        format!(
            "backend: {} ({})",
            report.backend,
            state_label(report.readiness.state())
        ),
        format!("project: {}", report.project_slug),
        format!(
            "supported backends: {}",
            match report.supported_backends.as_deref() {
                Some(backends) if !backends.is_empty() => backends
                    .iter()
                    .map(|backend| crate::config::escaped(backend))
                    .collect::<Vec<_>>()
                    .join(", "),
                _ => "any".to_string(),
            }
        ),
    ];
    lines.push(format!(
        "store view: {}",
        match report.store_view {
            StoreView::Closure => "closure",
            StoreView::Whole => "whole",
        }
    ));
    lines.extend(report.readiness.checks.iter().map(check_line));
    lines.extend(report.capabilities.iter().map(capability_line));
    for notice in &report.notices {
        lines.push(format!("notice: {notice}"));
    }
    if let Some(error) = &report.error {
        lines.push(format!("provisioning ({}): {}", error.class, error.message));
    }
    for step in &report.steps {
        let subject = match (&step.identity, step.kind.as_deref()) {
            (Some(identity), Some(kind)) => format!("step {identity} ({kind})"),
            (Some(identity), None) => format!("step {identity}"),
            (None, Some(kind)) => format!("step ({kind})"),
            (None, None) => "step".to_string(),
        };
        let mut line = format!("{subject}: {}", doctor_state_label(step.state));
        if let Some(class) = &step.class {
            line.push_str(&format!(" [{}]", class.as_str()));
        }
        if let Some(reason) = &step.reason {
            line.push_str(&format!(" ({reason})"));
        }
        if let Some(remedy) = &step.remedy {
            line.push_str(&format!(" -- {remedy}"));
        }
        lines.push(line);
        if let Some(installable) = &step.installable {
            lines.push(format!("    installable: {installable}"));
        }
        if let Some(count) = step.closure_count {
            lines.push(format!("    closure: {count} paths"));
        }
        for requirement in &step.requirements {
            let mut line = format!(
                "    {}: {}",
                requirement.name,
                if requirement.ok { "ok" } else { "MISSING" }
            );
            if !requirement.ok {
                line.push_str(&format!(" ({})", requirement.detail));
            }
            if let Some(remedy) = &requirement.remedy {
                line.push_str(&format!(" -- {remedy}"));
            }
            lines.push(line);
        }
    }
    Ok(lines.join("\n"))
}

fn check_line(check: &crate::sandbox::report::Check) -> String {
    let mut line = format!("    {}: {}", check.primitive, state_label(check.state));
    if let Some(reason) = &check.reason {
        line.push_str(&format!(" ({reason})"));
    }
    if let Some(remedy) = &check.remedy {
        line.push_str(&format!(" -- {remedy}"));
    }
    line
}

fn capability_line(capability: &crate::sandbox::report::Capability) -> String {
    let mut line = format!(
        "    {}: {}",
        capability.operation,
        state_label(capability.state)
    );
    if let Some(reason) = &capability.reason {
        line.push_str(&format!(" ({reason})"));
    }
    if let Some(remedy) = &capability.remedy {
        line.push_str(&format!(" -- {remedy}"));
    }
    line
}

fn doctor_state_label(state: DoctorState) -> &'static str {
    match state {
        DoctorState::Available => "available",
        DoctorState::Degraded => "degraded",
        DoctorState::Unavailable => "unavailable",
        DoctorState::Denied => "denied",
        DoctorState::Failed => "failed",
    }
}

/// Render the backend report as a table or as JSON, so a test can read it.
fn render_backends(reports: &[backends::BackendReport], json: bool) -> Result<String, Error> {
    if json {
        return serde_json::to_string_pretty(reports)
            .map_err(|error| Error::failed(error.to_string()));
    }

    Ok(reports
        .iter()
        .flat_map(report_lines)
        .collect::<Vec<String>>()
        .join("\n"))
}

/// The human-readable lines for one backend report.
fn report_lines(report: &backends::BackendReport) -> Vec<String> {
    let marker = if report.selected { "*" } else { " " };

    if let Some(error) = &report.error {
        return vec![format!("{marker} {} unavailable ({error})", report.name)];
    }

    let readiness = report.readiness.as_ref().expect("readiness or error");
    let mut lines = vec![format!(
        "{marker} {} {}",
        report.name,
        state_label(readiness.state())
    )];

    lines.extend(readiness.checks.iter().map(check_line));
    lines.extend(report.capabilities.iter().map(capability_line));

    lines
}

fn state_label(state: crate::sandbox::report::State) -> &'static str {
    match state {
        crate::sandbox::report::State::Available => "available",
        crate::sandbox::report::State::Degraded => "degraded",
        crate::sandbox::report::State::Unavailable => "unavailable",
    }
}

const EXIT_CODES: &[(u8, &str)] = &[
    (0, "success"),
    (1, "failed"),
    (2, "usage"),
    (3, "not found"),
    (4, "conflict"),
    (5, "unsupported"),
    (6, "denied"),
    (7, "unavailable"),
];

fn handle_docgen(kind: DocgenCommand) -> ExitCode {
    let content = match kind {
        DocgenCommand::Cli => {
            let mut content =
                agentkit_docgen::generate_cli_docs_with_exit_codes(&Cli::command(), EXIT_CODES);
            content.push_str(&crate::docgen::cli_section());
            content
        }
        DocgenCommand::Mcp => mcp::generate_mcp_docs(),
    };
    print!("{content}");
    ExitCode::from(0)
}

fn listing_failed(status: &SandboxStatus) -> bool {
    matches!(status, SandboxStatus::Error { .. } | SandboxStatus::Unknown)
}

fn status_label(status: &SandboxStatus) -> String {
    match status {
        SandboxStatus::Active => "active".to_string(),
        SandboxStatus::Paused => "paused".to_string(),
        SandboxStatus::Missing => "missing".to_string(),
        SandboxStatus::Unknown => "unknown".to_string(),
        SandboxStatus::Error { reason } => format!("error: {reason}"),
    }
}

fn report(error: Error) -> ExitCode {
    eprintln!("{error}");
    ExitCode::from(error.exit_class().code())
}

/// Report a failure that happened before the backend could be described.
///
/// `--json` promises a parseable report on every path, so an early failure is
/// rendered in the same shape with the facts that were never available left
/// empty and the class and message in `error`.
fn report_doctor_failure(error: Error, json: bool, backend: Option<&str>) -> ExitCode {
    if !json {
        return report(error);
    }
    let doctor_report = doctor_failure_report(&error, backend);
    match render_doctor(&doctor_report, true) {
        Ok(text) => {
            println!("{text}");
            ExitCode::from(error.exit_class().code())
        }
        Err(render_error) => report(render_error),
    }
}

/// The structured report for a failure before the backend could be described.
fn doctor_failure_report(error: &Error, backend: Option<&str>) -> DoctorReport {
    let backend = backend.unwrap_or("unknown").to_string();
    let message = error.to_string();
    DoctorReport {
        project_slug: String::new(),
        readiness: Readiness {
            backend: backend.clone(),
            checks: vec![crate::sandbox::report::Check {
                primitive: "preflight".to_string(),
                state: State::Unavailable,
                reason: Some(message.clone()),
                remedy: None,
                fallback: None,
            }],
        },
        capabilities: Vec::new(),
        supported_backends: None,
        notices: Vec::new(),
        store_view: StoreView::Closure,
        support: Vec::new(),
        steps: Vec::new(),
        error: Some(DoctorError {
            class: error.class().as_str().to_string(),
            message,
        }),
        backend,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DoctorError, DoctorReport, DoctorState, DoctorStep, build_doctor_report, doctor_exit_code,
        doctor_failure_report, listing_failed, render_backends, render_doctor, shell_exit_code,
        utc_time,
    };
    use crate::backends;
    use crate::config::{Config, ProjectConfig};
    use crate::sandbox::contract::{
        ContributionSort, ContributionSupport, Error, ProvisionStep, ProvisioningClass,
        SandboxStatus, StoreView,
    };
    use crate::sandbox::report::{Capability, Check, Fallback, Readiness, State};

    #[test]
    fn unix_seconds_format_as_utc() {
        assert_eq!(utc_time(0), "1970-01-01 00:00:00Z");
        assert_eq!(utc_time(1_700_000_000), "2023-11-14 22:13:20Z");
        assert_eq!(utc_time(-1), "1969-12-31 23:59:59Z");
    }

    fn support(state: State) -> Vec<ContributionSupport> {
        vec![ContributionSupport {
            sort: ContributionSort::Environment,
            state,
            reason: Some("the mechanism is degraded".to_string()),
            remedy: Some("accept it".to_string()),
            fallback: None,
        }]
    }

    fn command(identity: &str, run: &str) -> ProvisionStep {
        ProvisionStep {
            identity: identity.to_string(),
            kind: "command".to_string(),
            settings: [("run".to_string(), toml::Value::String(run.to_string()))].into(),
        }
    }

    fn devshell(identity: &str, installable: &str) -> ProvisionStep {
        ProvisionStep {
            identity: identity.to_string(),
            kind: "devshell".to_string(),
            settings: [(
                "installable".to_string(),
                toml::Value::String(installable.to_string()),
            )]
            .into(),
        }
    }

    fn provisioning_config() -> Config {
        Config {
            project: ProjectConfig {
                slug: Some("project".to_string()),
                backends: Some(vec!["docker".to_string()]),
            },
            provision_definitions: vec![command("setup", "true")],
            backend_sequences: [("docker".to_string(), vec!["setup".to_string()])]
                .into_iter()
                .collect(),
            ..Config::default()
        }
    }

    fn readiness() -> Readiness {
        Readiness {
            backend: "docker".to_string(),
            checks: Vec::new(),
        }
    }

    #[test]
    fn an_inspection_error_or_unknown_status_fails_list() {
        assert!(listing_failed(&SandboxStatus::Error {
            reason: "the daemon stopped".to_string()
        }));
        assert!(listing_failed(&SandboxStatus::Unknown));
        assert!(!listing_failed(&SandboxStatus::Active));
        assert!(!listing_failed(&SandboxStatus::Paused));
        assert!(!listing_failed(&SandboxStatus::Missing));
    }

    #[test]
    fn a_sandboxed_exit_status_cannot_collide_with_the_taxonomy() {
        assert_eq!(shell_exit_code(0), 0);
        assert_eq!(shell_exit_code(1), 129);
        assert_eq!(shell_exit_code(4), 132, "4 is conflict in the taxonomy");
        assert_eq!(shell_exit_code(127), 255);
        assert_eq!(shell_exit_code(-1), 129, "a signal is clamped into range");
    }

    #[tokio::test]
    async fn the_backend_table_names_the_selected_backend() {
        let reports = backends::introspect(&Config::default()).await;
        let table = render_backends(&reports, false).expect("renders");
        assert!(table.starts_with("* docker"), "{table}");
    }

    #[tokio::test]
    async fn the_backend_json_carries_the_selected_flag() {
        let reports = backends::introspect(&Config::default()).await;
        let json = render_backends(&reports, true).expect("renders");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        let docker = parsed
            .as_array()
            .expect("an array")
            .iter()
            .find(|entry| entry["name"] == "docker")
            .expect("docker is reported");
        assert_eq!(docker["selected"], true);
    }

    #[test]
    fn the_doctor_report_names_each_step_and_its_state() {
        let report = DoctorReport {
            backend: "docker".to_string(),
            project_slug: "project".to_string(),
            readiness: Readiness {
                backend: "docker".to_string(),
                checks: Vec::new(),
            },
            capabilities: vec![Capability {
                operation: "provision:command".to_string(),
                state: State::Available,
                reason: None,
                remedy: None,
                fallback: None,
            }],
            supported_backends: Some(vec!["docker".to_string()]),
            notices: Vec::new(),
            store_view: StoreView::Closure,
            support: Vec::new(),
            steps: vec![DoctorStep {
                identity: Some("fetch".to_string()),
                kind: Some("command".to_string()),
                state: DoctorState::Available,
                class: None,
                reason: None,
                remedy: None,
                installable: None,
                closure_count: None,
                requirements: Vec::new(),
            }],
            error: None,
        };

        let text = render_doctor(&report, false).expect("renders");
        assert!(text.contains("docker"), "{text}");
        assert!(text.contains("supported backends: docker"), "{text}");
        assert!(text.contains("fetch"), "{text}");
        assert!(text.contains("provision:command"), "{text}");
        assert!(text.contains("available"), "{text}");
    }

    #[test]
    fn the_doctor_text_names_an_unrestricted_backend_set() {
        let report = DoctorReport {
            backend: "docker".to_string(),
            project_slug: "project".to_string(),
            readiness: Readiness {
                backend: "docker".to_string(),
                checks: Vec::new(),
            },
            capabilities: Vec::new(),
            supported_backends: None,
            notices: Vec::new(),
            store_view: StoreView::Closure,
            support: Vec::new(),
            steps: Vec::new(),
            error: None,
        };

        let text = render_doctor(&report, false).expect("renders");
        assert!(text.contains("supported backends: any"), "{text}");
    }

    #[test]
    fn an_early_failure_still_renders_a_json_report() {
        let error = Error::Misconfigured {
            message: "bad configuration".to_string(),
        };
        let report = doctor_failure_report(&error, Some("docker"));
        let json = render_doctor(&report, true).expect("renders");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(parsed["backend"], "docker");
        assert_eq!(parsed["project_slug"], "");
        assert_eq!(parsed["error"]["class"], "misconfigured");
        assert!(
            parsed["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("bad configuration")),
            "{parsed}"
        );
        assert_eq!(parsed["readiness"]["checks"][0]["state"], "Unavailable");
        assert_eq!(parsed["steps"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn a_doctor_step_without_a_kind_is_not_labelled_command() {
        let report = DoctorReport {
            backend: "docker".to_string(),
            project_slug: "project".to_string(),
            readiness: Readiness {
                backend: "docker".to_string(),
                checks: Vec::new(),
            },
            capabilities: Vec::new(),
            supported_backends: None,
            notices: Vec::new(),
            store_view: StoreView::Closure,
            support: Vec::new(),
            steps: vec![DoctorStep {
                identity: None,
                kind: None,
                state: DoctorState::Unavailable,
                class: Some(ProvisioningClass::Unavailable),
                reason: None,
                remedy: None,
                installable: None,
                closure_count: None,
                requirements: Vec::new(),
            }],
            error: None,
        };

        let text = render_doctor(&report, false).expect("renders");
        assert!(text.contains("step: unavailable"), "{text}");
    }

    #[test]
    fn the_kind_registry_names_only_known_kinds() {
        assert!(crate::provision::kind::registry::lookup("command").is_some());
        assert!(crate::provision::kind::registry::lookup("nix").is_none());
        assert!(crate::provision::kind::registry::lookup("").is_none());
    }

    #[test]
    fn a_denied_step_and_a_structured_error_are_reported() {
        let report = DoctorReport {
            backend: "docker".to_string(),
            project_slug: "project".to_string(),
            readiness: Readiness {
                backend: "docker".to_string(),
                checks: Vec::new(),
            },
            capabilities: Vec::new(),
            supported_backends: Some(vec!["docker".to_string()]),
            notices: Vec::new(),
            store_view: StoreView::Closure,
            support: Vec::new(),
            steps: vec![DoctorStep {
                identity: Some("setup".to_string()),
                kind: Some("command".to_string()),
                state: DoctorState::Denied,
                class: Some(ProvisioningClass::Denied),
                reason: Some("consent is missing".to_string()),
                remedy: Some("set allowed-projects".to_string()),
                installable: None,
                closure_count: None,
                requirements: Vec::new(),
            }],
            error: Some(DoctorError {
                class: "misconfigured".to_string(),
                message: "the project does not list docker".to_string(),
            }),
        };

        let text = render_doctor(&report, false).expect("renders");
        assert!(text.contains("denied"), "{text}");
        assert!(text.contains("misconfigured"), "{text}");
    }

    #[test]
    fn the_doctor_builder_marks_a_command_step_denied_without_mutating() {
        let config = provisioning_config();
        let before = config.clone();
        let (report, failure) = build_doctor_report(
            &config,
            "project",
            "docker",
            readiness(),
            Vec::new(),
            &support(State::Available),
            None,
            Vec::new(),
        );
        assert_eq!(config, before, "the builder does not mutate the config");
        assert!(failure.is_none());
        assert_eq!(report.steps.len(), 1);
        assert_eq!(report.steps[0].state, DoctorState::Denied);
        assert!(report.steps[0].reason.is_some());
        assert!(
            report.steps[0]
                .remedy
                .as_deref()
                .is_some_and(|remedy| remedy.contains("allowed-projects")),
            "{:?}",
            report.steps[0]
        );
        assert_eq!(
            doctor_exit_code(&report, failure, config.accept_degraded),
            agentkit_common::exit::ExitClass::Denied.code()
        );
    }

    #[test]
    fn a_denied_step_is_reported_in_json_with_its_notices() {
        let mut config = provisioning_config();
        config
            .notices
            .push("a warning the doctor must carry".to_string());
        let (report, failure) = build_doctor_report(
            &config,
            "project",
            "docker",
            readiness(),
            Vec::new(),
            &support(State::Available),
            None,
            Vec::new(),
        );
        let json = render_doctor(&report, true).expect("renders");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(parsed["backend"], "docker");
        assert_eq!(parsed["supported_backends"][0], "docker");
        assert_eq!(parsed["notices"][0], "a warning the doctor must carry");
        assert_eq!(parsed["steps"][0]["identity"], "setup");
        assert_eq!(parsed["steps"][0]["kind"], "command");
        assert_eq!(parsed["steps"][0]["state"], "denied");
        assert_eq!(
            doctor_exit_code(&report, failure, config.accept_degraded),
            agentkit_common::exit::ExitClass::Denied.code()
        );
    }

    #[test]
    fn a_resolver_error_sets_the_class_and_the_exit_code() {
        let mut config = provisioning_config();
        config.project.backends = Some(vec!["other".to_string()]);
        let (report, failure) = build_doctor_report(
            &config,
            "project",
            "docker",
            readiness(),
            Vec::new(),
            &support(State::Available),
            None,
            Vec::new(),
        );
        let error = report.error.as_ref().expect("a structured error");
        assert_eq!(error.class, "misconfigured");
        assert!(error.message.contains("docker"), "{}", error.message);
        assert_eq!(
            doctor_exit_code(&report, failure, config.accept_degraded),
            agentkit_common::exit::ExitClass::Usage.code()
        );
    }

    #[test]
    fn a_ready_project_exits_zero() {
        let mut config = provisioning_config();
        config.allow_project_setup = true;
        let (report, failure) = build_doctor_report(
            &config,
            "project",
            "docker",
            readiness(),
            Vec::new(),
            &support(State::Available),
            None,
            Vec::new(),
        );
        assert_eq!(report.steps[0].state, DoctorState::Available);
        assert_eq!(
            doctor_exit_code(&report, failure, config.accept_degraded),
            0
        );
    }

    #[test]
    fn two_steps_keep_the_list_order() {
        let mut config = provisioning_config();
        config.allow_project_setup = true;
        config.provision_definitions =
            vec![command("z-first", "true"), command("a-second", "true")];
        config.backend_sequences.insert(
            "docker".to_string(),
            vec!["z-first".to_string(), "a-second".to_string()],
        );
        let (report, _failure) = build_doctor_report(
            &config,
            "project",
            "docker",
            readiness(),
            Vec::new(),
            &support(State::Available),
            None,
            Vec::new(),
        );
        let identities: Vec<&str> = report
            .steps
            .iter()
            .filter_map(|step| step.identity.as_deref())
            .collect();
        assert_eq!(identities, vec!["z-first", "a-second"]);
        assert!(
            report
                .steps
                .iter()
                .all(|step| step.state == DoctorState::Available)
        );
    }

    #[test]
    fn a_not_ready_step_without_a_class_exits_one() {
        let report = DoctorReport {
            backend: "docker".to_string(),
            project_slug: "project".to_string(),
            readiness: readiness(),
            capabilities: Vec::new(),
            supported_backends: None,
            notices: Vec::new(),
            store_view: StoreView::Closure,
            support: Vec::new(),
            steps: vec![DoctorStep {
                identity: Some("fetch".to_string()),
                kind: Some("command".to_string()),
                state: DoctorState::Unavailable,
                class: Some(ProvisioningClass::Unavailable),
                reason: None,
                remedy: None,
                installable: None,
                closure_count: None,
                requirements: Vec::new(),
            }],
            error: None,
        };
        assert_eq!(doctor_exit_code(&report, None, false), 1);
    }

    #[test]
    fn an_unavailable_backend_exits_with_the_unavailable_class() {
        let report = DoctorReport {
            backend: "docker".to_string(),
            project_slug: "project".to_string(),
            readiness: Readiness {
                backend: "docker".to_string(),
                checks: vec![Check {
                    primitive: "the Docker daemon".to_string(),
                    state: State::Unavailable,
                    reason: Some("the daemon is down".to_string()),
                    remedy: Some("start it".to_string()),
                    fallback: None,
                }],
            },
            capabilities: Vec::new(),
            supported_backends: None,
            notices: Vec::new(),
            store_view: StoreView::Closure,
            support: Vec::new(),
            steps: Vec::new(),
            error: None,
        };
        assert_eq!(
            doctor_exit_code(&report, None, false),
            agentkit_common::exit::ExitClass::Unavailable.code()
        );
    }

    #[test]
    fn a_degraded_fallback_needs_acceptance_to_exit_zero() {
        let report = DoctorReport {
            backend: "docker".to_string(),
            project_slug: "project".to_string(),
            readiness: Readiness {
                backend: "docker".to_string(),
                checks: vec![Check {
                    primitive: "a namespace".to_string(),
                    state: State::Degraded,
                    reason: Some("a weaker mode".to_string()),
                    remedy: Some("accept it".to_string()),
                    fallback: Some(Fallback {
                        name: "a weaker namespace".to_string(),
                        does_not_bound: "the host network".to_string(),
                    }),
                }],
            },
            capabilities: Vec::new(),
            supported_backends: None,
            notices: Vec::new(),
            store_view: StoreView::Closure,
            support: Vec::new(),
            steps: Vec::new(),
            error: None,
        };
        assert_eq!(
            doctor_exit_code(&report, None, false),
            agentkit_common::exit::ExitClass::Unavailable.code()
        );
        assert_eq!(doctor_exit_code(&report, None, true), 0);
    }

    #[test]
    fn an_unavailable_contribution_sets_the_class_and_the_exit_code() {
        let mut config = provisioning_config();
        config.allow_project_setup = true;
        config.provision_definitions = vec![devshell("dev", "./nix")];
        config
            .backend_sequences
            .insert("docker".to_string(), vec!["dev".to_string()]);
        let (report, failure) = build_doctor_report(
            &config,
            "project",
            "docker",
            readiness(),
            Vec::new(),
            &support(State::Unavailable),
            None,
            Vec::new(),
        );
        assert_eq!(report.steps.len(), 1);
        assert_eq!(report.steps[0].state, DoctorState::Unavailable);
        assert_eq!(report.steps[0].identity.as_deref(), Some("dev"));
        assert_eq!(report.steps[0].kind.as_deref(), Some("devshell"));
        let error = report.error.as_ref().expect("a structured error");
        assert_eq!(error.class, "unsupported");
        assert_eq!(
            doctor_exit_code(&report, failure, config.accept_degraded),
            agentkit_common::exit::ExitClass::Unsupported.code()
        );
    }
}
