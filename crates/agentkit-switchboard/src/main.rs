use agentkit_switchboard::auth;
use agentkit_switchboard::cli::{AuthCommands, Cli, Commands};
use agentkit_switchboard::config;
use clap::Parser;

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
        Ok(mut cfg) => {
            if let Some(path) = cli.session_db.clone() {
                cfg.session_db_path = Some(path);
            }
            if let Some(helper) = cli.credential_helper.clone() {
                cfg.credential_helper = Some(helper);
            }
            match &cli.command {
                Some(Commands::Auth(auth_cmd)) => {
                    let cmd = match auth_cmd {
                        AuthCommands::Login { identity } => auth::AuthCommand::Login {
                            identity: identity.clone(),
                        },
                        AuthCommands::Add { identity, value } => auth::AuthCommand::Add {
                            identity: identity.clone(),
                            value: value.clone(),
                        },
                        AuthCommands::Status { identity } => auth::AuthCommand::Status {
                            identity: identity.clone(),
                        },
                        AuthCommands::Token { identity } => auth::AuthCommand::Token {
                            identity: identity.clone(),
                        },
                        AuthCommands::Logout { identity } => auth::AuthCommand::Logout {
                            identity: identity.clone(),
                        },
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
                    match agentkit_switchboard::server::start(
                        cfg,
                        &cli.bind,
                        cli.port,
                        cli.models_db.clone(),
                    )
                    .await
                    {
                        Ok(_) => std::process::ExitCode::SUCCESS,
                        Err(e) => {
                            tracing::error!("server error: {e}");
                            std::process::ExitCode::FAILURE
                        }
                    }
                }
            }
        }
        Err(e) => {
            tracing::error!("{e}");
            std::process::ExitCode::FAILURE
        }
    }
}
