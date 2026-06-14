use clap::Parser;
use agentkit_switchboard::cli::{Cli, Commands, AuthCommands};
use agentkit_switchboard::config;
use agentkit_switchboard::auth;

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
            match &cli.command {
                Some(Commands::Auth(auth_cmd)) => {
                    let cmd = match auth_cmd {
                        AuthCommands::Login { identity } => auth::AuthCommand::Login { identity: identity.clone() },
                        AuthCommands::Status { identity } => auth::AuthCommand::Status { identity: identity.clone() },
                        AuthCommands::Token { identity } => auth::AuthCommand::Token { identity: identity.clone() },
                        AuthCommands::Logout { identity } => auth::AuthCommand::Logout { identity: identity.clone() },
                    };
                    match auth::handle_auth(cmd, &cfg).await {
                        Ok(msg) => {
                            println!("{msg}");
                            std::process::ExitCode::SUCCESS
                        }
                        Err(e) => {
                            eprintln!("error: {e}");
                            std::process::ExitCode::FAILURE
                        }
                    }
                }
                Some(Commands::Start) | None => {
                    tracing::info!(
                        "switchboard starting with {} providers",
                        cfg.providers.len()
                    );
                    std::process::ExitCode::SUCCESS
                }
            }
        }
        Err(e) => {
            tracing::error!("{e}");
            std::process::ExitCode::FAILURE
        }
    }
}
