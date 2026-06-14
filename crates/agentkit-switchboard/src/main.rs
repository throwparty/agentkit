mod cli;
mod config;

use clap::Parser;
use cli::Cli;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("switchboard={}", cli.log_level).into()),
        )
        .init();

    match config::loader::load_config(&cli.config) {
        Ok(cfg) => {
            tracing::info!(
                "switchboard starting with {} providers",
                cfg.providers.len()
            );
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            tracing::error!("{e}");
            std::process::ExitCode::FAILURE
        }
    }
}
