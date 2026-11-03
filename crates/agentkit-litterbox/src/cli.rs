use std::process::ExitCode;

use crate::backends;
use crate::config::Config;
use crate::config_loader;
use crate::mcp;
use crate::orchestrator::Sandboxes;
use crate::sandbox::contract::{Error, SandboxStatus};
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

        /// Delete even if the sandbox is active
        #[arg(short, long)]
        force: bool,

        /// Remove a record written by an earlier release instead of a sandbox
        #[arg(long)]
        discard_legacy: bool,
    },

    /// Report the backends available on this machine, their readiness and their
    /// capabilities.
    Backends {
        /// Emit structured JSON instead of a table.
        #[arg(long)]
        json: bool,
    },

    /// Execute a command in a sandbox
    Shell {
        /// Name of the sandbox to run the command in
        name: String,

        /// Command and arguments to execute
        #[arg(required = true, trailing_var_arg = true)]
        command: Vec<String>,
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
            force,
            discard_legacy,
        } => handle_delete(backend, name, force, discard_legacy).await,
        Commands::Shell { name, command } => handle_shell(backend, name, command).await,
        Commands::Backends { json } => handle_backends(backend, json).await,
        Commands::Docgen { kind } => handle_docgen(kind),
    }
}

fn load_config(backend: Option<String>) -> Result<Config, Error> {
    let mut config = config_loader::load_final().map_err(|error| Error::Misconfigured {
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
    Sandboxes::open(config).map_err(Error::from)
}

async fn handle_stdio(backend: Option<String>) -> ExitCode {
    if let Err(error) = mcp::run_stdio(backend).await {
        return report(Error::failed(error.to_string()));
    }
    ExitCode::from(0)
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

    for name in &listing.legacy {
        println!(
            "{name} legacy-record; run `litterbox delete --discard-legacy {name}` to remove it"
        );
    }
    let mut failed = false;
    for entry in &listing.corrupt {
        println!("{entry}");
        failed = true;
    }
    for name in &listing.orphans {
        println!(
            "{name} orphan; the branch has no record, so the compute it was paired with \
             cannot be addressed by Litterbox"
        );
    }
    let inspections = match sandboxes.inspect_listing(listing).await {
        Ok(inspections) => inspections,
        Err(error) => return report(error),
    };
    for (name, inspection) in inspections {
        match inspection {
            Ok(inspection) => {
                println!(
                    "{} {} {}",
                    name,
                    status_label(&inspection.status),
                    inspection.backend
                );
                if listing_failed(&inspection.status) {
                    failed = true;
                }
            }
            Err(error) => {
                println!("{name} unknown {error}");
                failed = true;
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

async fn handle_delete(
    backend: Option<String>,
    name: String,
    force: bool,
    discard_legacy: bool,
) -> ExitCode {
    let config = match load_config(backend) {
        Ok(config) => config,
        Err(error) => return report(error),
    };
    let sandboxes = match project(config) {
        Ok(sandboxes) => sandboxes,
        Err(error) => return report(error),
    };

    if discard_legacy {
        return match sandboxes.discard_legacy(&name) {
            Ok((path, container)) => {
                println!("Discarded the record at {}", path.display());
                if let Some(container) = container {
                    println!(
                        "The record named compute '{container}'; remove it with your \
                         backend's tooling before the name is reused"
                    );
                }
                ExitCode::from(0)
            }
            Err(error) => report(error),
        };
    }

    match sandboxes.status(&name).await {
        Ok(SandboxStatus::Active) if !force => {
            return report(Error::Denied {
                operation: format!("delete sandbox '{name}'"),
                reason: "it is active; pass --force to delete it".to_string(),
            });
        }
        Ok(_) | Err(Error::NotFound { .. }) => {}
        Err(error) => return report(error),
    }

    match sandboxes.delete(&name).await {
        Ok(()) => {
            println!("Deleted {name}");
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

    let result = match sandboxes.exec(&name, &command).await {
        Ok(result) => result,
        Err(error) => return report(error),
    };

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

    for check in &readiness.checks {
        let mut line = format!("    {}: {}", check.primitive, state_label(check.state));
        if let Some(reason) = &check.reason {
            line.push_str(&format!(" ({reason})"));
        }
        if let Some(remedy) = &check.remedy {
            line.push_str(&format!(" -- {remedy}"));
        }
        lines.push(line);
    }
    for capability in &report.capabilities {
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
        lines.push(line);
    }

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
            agentkit_docgen::generate_cli_docs_with_exit_codes(&Cli::command(), EXIT_CODES)
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

#[cfg(test)]
mod tests {
    use super::{listing_failed, render_backends, shell_exit_code};
    use crate::backends;
    use crate::config::Config;
    use crate::sandbox::contract::SandboxStatus;

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
}
