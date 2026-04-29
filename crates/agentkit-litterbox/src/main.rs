use std::path::Path;
use std::process::ExitCode;

use bollard::query_parameters::ListContainersOptionsBuilder;
use clap::{Arg, CommandFactory, Parser, Subcommand};
use agentkit_litterbox::compute::DockerCompute;
use agentkit_litterbox::domain::{ComputeError, SandboxError, SandboxMetadata, SandboxStatus, slugify_name};
use agentkit_litterbox::mcp;
use agentkit_litterbox::sandbox::{
    DockerSandboxProvider, SandboxProvider, branch_name_for_slug, container_name_for_slug,
};
use agentkit_litterbox::scm::{Scm, ThreadSafeScm};

#[derive(Parser)]
#[command(
    author,
    version,
    about = "Litterbox - sandboxing tool for software engineering agents",
    long_about = "Litterbox provides isolated sandbox environments combining SCM branches and containerized compute.\n\
                  Each sandbox pairs a Git branch with a Docker container for safe, isolated development work."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List all sandboxes in the current repository
    ///
    /// Shows all sandboxes with their current status (active, paused, missing, or error).
    /// Status information requires Docker to be available; otherwise statuses show as unknown.
    List,

    /// Run the MCP (Model Control Protocol) server over stdio
    ///
    /// Starts the Litterbox MCP server, enabling communication with AI agents and tools
    /// that support the Model Control Protocol. The server uses standard input/output
    /// for communication.
    Stdio,

    /// Pause one or more sandboxes
    ///
    /// Pauses the container(s) associated with sandbox(es), preserving their state
    /// while freeing up system resources. Paused sandboxes can be resumed later.
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
            help = "Pause all Litterbox containers system-wide"
        )]
        all_repos: bool,
    },

    /// Resume a paused sandbox
    ///
    /// Resumes a previously paused sandbox, restoring its container to an active state.
    Resume {
        /// Name of the sandbox to resume
        name: String,
    },

    /// Delete a sandbox
    ///
    /// Removes both the sandbox's Git branch and container. Active sandboxes require
    /// the --force flag to prevent accidental deletion.
    Delete {
        /// Name of the sandbox to delete
        name: String,

        /// Force deletion even if the sandbox is active
        #[arg(short, long)]
        force: bool,
    },

    /// Execute a shell command in a sandbox
    ///
    /// Runs the specified command inside the sandbox's container and returns the output.
    /// The command is executed in the sandbox's working directory.
    Shell {
        /// Name of the sandbox to run the command in
        name: String,

        /// Command and arguments to execute
        #[arg(required = true, trailing_var_arg = true)]
        command: Vec<String>,
    },

    /// Generate reference documentation
    ///
    /// Prints docs to stdout.
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
    match cli.command {
        Commands::List => handle_list().await,
        Commands::Stdio => handle_stdio().await,
        Commands::Pause {
            name,
            all_envs,
            all_repos,
        } => handle_pause(name, all_envs, all_repos).await,
        Commands::Resume { name } => handle_resume(name).await,
        Commands::Delete { name, force } => handle_delete(name, force).await,
        Commands::Shell { name, command } => handle_shell(name, command).await,
        Commands::Docgen { kind } => handle_docgen(kind),
    }
}

async fn handle_stdio() -> ExitCode {
    // Load and print config for debugging
    match agentkit_litterbox::config_loader::load_final() {
        Ok(config) => {
            eprintln!("Loaded configuration: {:#?}", config);
        }
        Err(error) => {
            eprintln!("Warning: Failed to load config: {}", error);
        }
    }

    if let Err(error) = mcp::run_stdio().await {
        return report_error("stdio", error);
    }
    ExitCode::from(0)
}

async fn handle_list() -> ExitCode {
    let scm = match ThreadSafeScm::open(Path::new(".")) {
        Ok(scm) => scm,
        Err(error) => return report_error("list", error),
    };
    let repo_prefix = match scm.repo_prefix() {
        Ok(prefix) => prefix,
        Err(error) => return report_error("list", error),
    };
    let slugs = match scm.list_sandboxes() {
        Ok(slugs) => slugs,
        Err(error) => return report_error("list", error),
    };

    let compute = match DockerCompute::connect() {
        Ok(compute) => Some(compute),
        Err(_) => {
            eprintln!("list warning: docker unavailable; statuses shown as unknown");
            None
        }
    };

    let mut sandboxes = Vec::new();
    for slug in slugs {
        let status = match compute.as_ref() {
            Some(compute) => {
                let container = container_name_for_slug(&repo_prefix, &slug);
                match compute.client().inspect_container(&container, None).await {
                    Ok(info) => {
                        let state = info.state.as_ref();
                        let running = state.and_then(|state| state.running).unwrap_or(false);
                        let paused = state.and_then(|state| state.paused).unwrap_or(false);
                        if paused {
                            SandboxStatus::Paused
                        } else if running {
                            SandboxStatus::Active
                        } else {
                            SandboxStatus::Error("not running".to_string())
                        }
                    }
                    Err(bollard::errors::Error::DockerResponseServerError {
                        status_code: 404,
                        ..
                    }) => SandboxStatus::Error("missing container".to_string()),
                    Err(error) => return report_error("list", error),
                }
            }
            None => SandboxStatus::Error("docker unavailable".to_string()),
        };
        sandboxes.push(metadata_for_slug(&repo_prefix, &slug, status));
    }

    sandboxes.sort_by(|a, b| a.name.cmp(&b.name));
    for sandbox in sandboxes {
        println!("{} {}", sandbox.name, status_label(&sandbox.status));
    }

    ExitCode::from(0)
}

async fn handle_pause(name: Option<String>, all_envs: bool, all_repos: bool) -> ExitCode {
    if all_repos {
        return handle_pause_all_repos().await;
    }
    if all_envs {
        return handle_pause_all_envs().await;
    }

    let Some(name) = name else {
        return report_error("pause", "missing sandbox name");
    };
    let slug = match slugify_name(&name) {
        Ok(slug) => slug,
        Err(error) => return report_error("pause", error),
    };
    let repo_prefix = match repo_prefix() {
        Ok(prefix) => prefix,
        Err(error) => return report_error("pause", error),
    };
    let container = container_name_for_slug(&repo_prefix, &slug);
    let provider = match build_provider() {
        Ok(provider) => provider,
        Err(error) => return report_error("pause", error),
    };
    if let Err(error) = provider.pause(&container).await {
        return report_error("pause", error);
    }
    let metadata = metadata_for_slug(&repo_prefix, &slug, SandboxStatus::Paused);
    println!("Paused {metadata}");
    ExitCode::from(0)
}

async fn handle_pause_all_envs() -> ExitCode {
    let scm = match ThreadSafeScm::open(Path::new(".")) {
        Ok(scm) => scm,
        Err(error) => return report_error("pause --all-envs", error),
    };
    let repo_prefix = match scm.repo_prefix() {
        Ok(prefix) => prefix,
        Err(error) => return report_error("pause --all-envs", error),
    };
    let compute = match DockerCompute::connect() {
        Ok(compute) => compute,
        Err(error) => return report_error("pause --all-envs", error),
    };
    let slugs = match scm.list_sandboxes() {
        Ok(slugs) => slugs,
        Err(error) => return report_error("pause --all-envs", error),
    };

    let mut paused = 0usize;
    for slug in slugs {
        let container = container_name_for_slug(&repo_prefix, &slug);
        match compute.pause_container(&container).await {
            Ok(()) => paused += 1,
            Err(error) if is_container_missing(&error) => {}
            Err(error) => return report_error("pause --all-envs", error),
        }
    }

    println!("Paused {paused} sandbox(es)");
    ExitCode::from(0)
}

async fn handle_pause_all_repos() -> ExitCode {
    let compute = match DockerCompute::connect() {
        Ok(compute) => compute,
        Err(error) => return report_error("pause --all-repos", error),
    };
    let options = Some(ListContainersOptionsBuilder::default().all(true).build());
    let containers = match compute.client().list_containers(options).await {
        Ok(containers) => containers,
        Err(error) => return report_error("pause --all-repos", error),
    };

    let mut paused = 0usize;
    for container in containers {
        let Some(names) = container.names.as_ref() else {
            continue;
        };
        if !names.iter().any(|name| name.starts_with("/litterbox-")) {
            continue;
        }
        let Some(id) = container.id.as_ref() else {
            continue;
        };
        let running = matches!(
            container.state,
            Some(bollard::models::ContainerSummaryStateEnum::RUNNING)
        );
        if !running {
            continue;
        }
        match compute.pause_container(id).await {
            Ok(()) => paused += 1,
            Err(error) if is_container_missing(&error) => {}
            Err(error) => return report_error("pause --all-repos", error),
        }
    }

    println!("Paused {paused} sandbox(es)");
    ExitCode::from(0)
}

async fn handle_resume(name: String) -> ExitCode {
    let slug = match slugify_name(&name) {
        Ok(slug) => slug,
        Err(error) => return report_error("resume", error),
    };
    let repo_prefix = match repo_prefix() {
        Ok(prefix) => prefix,
        Err(error) => return report_error("resume", error),
    };
    let container = container_name_for_slug(&repo_prefix, &slug);
    let provider = match build_provider() {
        Ok(provider) => provider,
        Err(error) => return report_error("resume", error),
    };
    if let Err(error) = provider.resume(&container).await {
        return report_error("resume", error);
    }
    let metadata = metadata_for_slug(&repo_prefix, &slug, SandboxStatus::Active);
    println!("Resumed {metadata}");
    ExitCode::from(0)
}

async fn handle_delete(name: String, force: bool) -> ExitCode {
    let slug = match slugify_name(&name) {
        Ok(slug) => slug,
        Err(error) => return report_error("delete", error),
    };
    let repo_prefix = match repo_prefix() {
        Ok(prefix) => prefix,
        Err(error) => return report_error("delete", error),
    };
    let container = container_name_for_slug(&repo_prefix, &slug);
    let compute = match DockerCompute::connect() {
        Ok(compute) => compute,
        Err(error) => return report_error("delete", error),
    };

    match compute.client().inspect_container(&container, None).await {
        Ok(info) => {
            let running = info
                .state
                .as_ref()
                .and_then(|state| state.running)
                .unwrap_or(false);
            let paused = info
                .state
                .as_ref()
                .and_then(|state| state.paused)
                .unwrap_or(false);
            if running && !paused && !force {
                return report_error("delete", "sandbox is active; use --force to delete");
            }
        }
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => {}
        Err(error) => return report_error("delete", error),
    }

    let provider = match build_provider() {
        Ok(provider) => provider,
        Err(error) => return report_error("delete", error),
    };
    let metadata = metadata_for_slug(&repo_prefix, &slug, SandboxStatus::Active);
    if let Err(error) = provider.delete(&metadata).await {
        return report_error("delete", error);
    }

    println!("Deleted {name}");
    ExitCode::from(0)
}

async fn handle_shell(name: String, command: Vec<String>) -> ExitCode {
    let slug = match slugify_name(&name) {
        Ok(slug) => slug,
        Err(error) => return report_error("shell", error),
    };
    let provider = match build_provider() {
        Ok(provider) => provider,
        Err(error) => return report_error("shell", error),
    };
    let repo_prefix = match repo_prefix() {
        Ok(prefix) => prefix,
        Err(error) => return report_error("shell", error),
    };
    let metadata = metadata_for_slug(&repo_prefix, &slug, SandboxStatus::Active);

    let result = match provider.shell(&metadata, &command).await {
        Ok(result) => result,
        Err(error) => return report_error("shell", error),
    };

    if !result.stdout.is_empty() {
        print!("{}", result.stdout);
    }
    if !result.stderr.is_empty() {
        eprint!("{}", result.stderr);
    }
    if result.exit_code != 0 {
        eprintln!("shell failed: {result}");
    }

    if result.exit_code == 0 {
        ExitCode::from(0)
    } else if let Ok(code) = u8::try_from(result.exit_code) {
        ExitCode::from(code)
    } else {
        ExitCode::from(1)
    }
}

fn handle_docgen(kind: DocgenCommand) -> ExitCode {
    let content = match kind {
        DocgenCommand::Cli => generate_cli_docs(),
        DocgenCommand::Mcp => mcp::generate_mcp_docs(),
    };
    print!("{content}");
    ExitCode::from(0)
}

fn generate_cli_docs() -> String {
    let mut command = Cli::command();
    command.build();

    let mut output = String::new();
    output.push_str("# 👩‍💻 Command line interface\n\n");

    let mut subcommands: Vec<_> = command.get_subcommands().collect();
    subcommands.sort_by(|a, b| a.get_name().cmp(b.get_name()));

    for subcommand in subcommands {
        output.push_str(&format!("## `{}`\n\n", subcommand.get_name()));

        if let Some(about) = subcommand
            .get_long_about()
            .or_else(|| subcommand.get_about())
        {
            output.push_str(&about.to_string());
            output.push_str("\n\n");
        }

        let (positionals, options): (Vec<&Arg>, Vec<&Arg>) = subcommand
            .get_arguments()
            .filter(|arg| !arg.is_hide_set())
            .partition(|arg| arg.is_positional());

        if !positionals.is_empty() {
            output.push_str("Arguments:\n\n");
            for arg in positionals {
                let label = format_positional_label(arg);
                output.push_str(&format!("- `{}`", label));
                if let Some(help) = arg.get_help().or_else(|| arg.get_long_help()) {
                    output.push_str(&format!(" {}", help));
                }
                output.push('\n');
            }
            output.push('\n');
        }

        if !options.is_empty() {
            output.push_str("Options:\n\n");
            for arg in options {
                if let Some(label) = format_option_label(arg) {
                    output.push_str(&format!("- `{}`", label));
                    if let Some(help) = arg.get_help().or_else(|| arg.get_long_help()) {
                        output.push_str(&format!(" {}", help));
                    }
                    output.push('\n');
                }
            }
            output.push('\n');
        }
    }

    output
}

fn format_positional_label(arg: &Arg) -> String {
    let name = arg
        .get_value_names()
        .map(|names| names.iter().map(|name| name.as_str()).collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| arg.get_id().as_str().to_string());

    if arg.is_required_set() {
        name
    } else {
        format!("[{name}]")
    }
}

fn format_option_label(arg: &Arg) -> Option<String> {
    let mut flags = Vec::new();
    if let Some(short) = arg.get_short() {
        flags.push(format!("-{short}"));
    }
    if let Some(long) = arg.get_long() {
        flags.push(format!("--{long}"));
    }
    if flags.is_empty() {
        return None;
    }

    let mut label = flags.join(", ");
    if let Some(value_name) = arg
        .get_value_names()
        .map(|names| names.iter().map(|name| name.as_str()).collect::<Vec<_>>().join(" "))
    {
        label.push_str(&format!(" <{value_name}>",));
    }

    Some(label)
}

fn build_provider() -> Result<DockerSandboxProvider<ThreadSafeScm, DockerCompute>, SandboxError> {
    let scm = ThreadSafeScm::open(Path::new("."))?;
    let compute = DockerCompute::connect()?;
    Ok(DockerSandboxProvider::new(scm, compute))
}

fn metadata_for_slug(repo_prefix: &str, slug: &str, status: SandboxStatus) -> SandboxMetadata {
    SandboxMetadata {
        name: slug.to_string(),
        branch_name: branch_name_for_slug(slug),
        container_id: container_name_for_slug(repo_prefix, slug),
        status,
        forwarded_ports: Vec::new(),
    }
}

fn report_error(action: &str, error: impl std::fmt::Display) -> ExitCode {
    eprintln!("{action} failed: {error}");
    ExitCode::from(1)
}

fn repo_prefix() -> Result<String, SandboxError> {
    ThreadSafeScm::open(Path::new("."))?.repo_prefix()
}

fn status_label(status: &SandboxStatus) -> String {
    match status {
        SandboxStatus::Active => "active".to_string(),
        SandboxStatus::Paused => "paused".to_string(),
        SandboxStatus::Error(message) if message == "missing container" => "missing".to_string(),
        SandboxStatus::Error(message) if message == "docker unavailable" => "unknown".to_string(),
        SandboxStatus::Error(message) => format!("error: {message}"),
    }
}

fn is_container_missing(error: &SandboxError) -> bool {
    matches!(
        error,
        SandboxError::Compute(ComputeError::ContainerPause {
            source: bollard::errors::Error::DockerResponseServerError {
                status_code: 404,
                ..
            }
        })
    )
}
