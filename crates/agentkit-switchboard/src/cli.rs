use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "switchboard",
    version,
    about = "Cost-aware model provider proxy"
)]
pub struct Cli {
    /// Path to the TOML configuration file (defaults to the platform config
    /// directory)
    #[arg(long)]
    pub config: Option<PathBuf>,

    #[arg(long, default_value = "127.0.0.1")]
    pub bind: String,

    #[arg(long, default_value_t = 3812)]
    pub port: u16,

    #[arg(long, default_value = "info")]
    pub log_level: String,

    #[arg(long)]
    pub session_db: Option<PathBuf>,

    #[arg(long)]
    pub models_db: Option<PathBuf>,

    #[arg(long)]
    pub credential_helper: Option<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// Resolves the effective configuration file path, honouring the CLI
/// override.
pub fn config_path(config: Option<&PathBuf>) -> PathBuf {
    config
        .cloned()
        .unwrap_or_else(|| agentkit_path::config_dir("switchboard").join("config.toml"))
}

#[derive(Subcommand)]
pub enum Commands {
    #[command(subcommand)]
    Auth(AuthCommands),
    Start,

    /// Generate reference documentation
    #[command(subcommand, hide = true)]
    Docgen(DocgenCommand),
}

#[derive(Subcommand)]
pub enum DocgenCommand {
    /// Generate CLI reference documentation
    Cli,
}

#[derive(Subcommand)]
pub enum AuthCommands {
    #[command(name = "login")]
    Login { identity: String },
    #[command(name = "add")]
    Add { identity: String, value: String },
    #[command(name = "status")]
    Status { identity: Option<String> },
    #[command(name = "token")]
    Token { identity: String },
    #[command(name = "logout")]
    Logout { identity: String },
}
