use std::path::PathBuf;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "switchboard", version, about = "Cost-aware model provider proxy")]
pub struct Cli {
    #[arg(long, required = true)]
    pub config: PathBuf,

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

#[derive(Subcommand)]
pub enum Commands {
    #[command(subcommand)]
    Auth(AuthCommands),
    Start,
}

#[derive(Subcommand)]
pub enum AuthCommands {
    #[command(name = "login")]
    Login { identity: String },
    #[command(name = "status")]
    Status { identity: Option<String> },
    #[command(name = "token")]
    Token { identity: String },
    #[command(name = "logout")]
    Logout { identity: String },
}
