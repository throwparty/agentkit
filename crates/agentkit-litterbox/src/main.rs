use std::process::ExitCode;

use agentkit_litterbox::backends;
use agentkit_litterbox::config::Config;
use agentkit_litterbox::config_loader;
use agentkit_litterbox::mcp;
use agentkit_litterbox::sandbox::contract::{Error, SandboxStatus, Scope};
use agentkit_litterbox::sandboxes::Sandboxes;
use agentkit_litterbox::sandboxes::bulk;
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
            required_unless_present_any = ["all_envs", "all_repos"],
            help = "Sandbox name to pause"
        )]
        name: Option<String>,

        /// Pause all sandboxes in the current repository
        #[arg(
            long,
            conflicts_with = "all_repos",
            help = "Pause all sandboxes in this repository"
        )]
        all_envs: bool,

        /// Pause all Litterbox sandboxes across all repositories
        #[arg(
            long,
            conflicts_with = "all_envs",
            help = "Pause all Litterbox sandboxes on this machine"
        )]
        all_repos: bool,
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

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let backend = cli.backend;
    match cli.command {
        Commands::List => handle_list(backend).await,
        Commands::Stdio => handle_stdio(backend).await,
        Commands::Pause {
            name,
            all_envs,
            all_repos,
        } => handle_pause(backend, name, all_envs, all_repos).await,
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
    let mut config =
        config_loader::load_final().map_err(|error| Error::failed(error.to_string()))?;
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

    println!(
        "records {}",
        agentkit_litterbox::metadata_store::directory(sandboxes.project_slug()).display()
    );

    for name in &listing.legacy {
        println!("{name} legacy-record");
    }
    for record in &listing.sandboxes {
        let status = sandboxes
            .status(&record.name)
            .await
            .unwrap_or(SandboxStatus::Unknown);
        println!(
            "{} {} {}",
            record.name,
            status_label(&status),
            record.backend
        );
    }

    ExitCode::from(0)
}

async fn handle_pause(
    backend: Option<String>,
    name: Option<String>,
    all_envs: bool,
    all_repos: bool,
) -> ExitCode {
    let config = match load_config(backend) {
        Ok(config) => config,
        Err(error) => return report(error),
    };

    if all_repos {
        return pause_everywhere(config).await;
    }

    let sandboxes = match project(config) {
        Ok(sandboxes) => sandboxes,
        Err(error) => return report(error),
    };

    if all_envs {
        let paused = match sandboxes.pause_listed().await {
            Ok(paused) => paused,
            Err(error) => return report(error),
        };
        println!("Paused {paused} sandbox(es)");
        return ExitCode::from(0);
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

async fn pause_everywhere(config: Config) -> ExitCode {
    let selected = match backends::select(&config).await {
        Ok(selected) => selected,
        Err(error) => return report(error),
    };
    let Some(lifecycle) = selected.lifecycle.as_ref() else {
        return report(Error::Unsupported {
            operation: "pause".to_string(),
            backend: backends::selected_name(&config).to_string(),
            reason: "the mechanism has no sandbox-level pause".to_string(),
            remedy: "stop the command instead".to_string(),
        });
    };

    let records = match selected.core.list(&Scope::Machine).await {
        Ok(records) => records,
        Err(error) => return report(error),
    };

    let paused = match bulk::pause_all(&records, lifecycle.as_ref()).await {
        Ok(paused) => paused,
        Err(error) => return report(error),
    };

    println!("Paused {paused} sandbox(es)");
    ExitCode::from(0)
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
            Ok(path) => {
                println!("Discarded the record at {}", path.display());
                ExitCode::from(0)
            }
            Err(error) => report(error),
        };
    }

    match sandboxes.status(&name).await {
        Ok(SandboxStatus::Active) if !force => {
            return report(Error::Conflict {
                name: format!("{name} is active; pass --force to delete it"),
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

    if result.exit_code == 0 {
        ExitCode::from(0)
    } else if let Ok(code) = u8::try_from(result.exit_code) {
        ExitCode::from(code)
    } else {
        ExitCode::from(1)
    }
}

async fn handle_backends(backend: Option<String>, json: bool) -> ExitCode {
    let mut config = match config_loader::load_user_only() {
        Ok(config) => config,
        Err(error) => return report(Error::failed(error.to_string())),
    };
    if let Some(backend) = backend {
        config.backend = Some(backend);
    }
    for notice in &config.notices {
        eprintln!("configuration: {notice}");
    }

    let reports = backends::introspect(&config).await;

    if json {
        match serde_json::to_string_pretty(&reports) {
            Ok(text) => println!("{text}"),
            Err(error) => return report(Error::failed(error.to_string())),
        }
        return ExitCode::from(0);
    }

    for report in &reports {
        for line in report.lines() {
            println!("{line}");
        }
    }

    ExitCode::from(0)
}

fn handle_docgen(kind: DocgenCommand) -> ExitCode {
    let content = match kind {
        DocgenCommand::Cli => agentkit_docgen::generate_cli_docs(&Cli::command()),
        DocgenCommand::Mcp => mcp::generate_mcp_docs(),
    };
    print!("{content}");
    ExitCode::from(0)
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
