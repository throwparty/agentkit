//! Tackle: ACP-native agentic coding harness. Binary entrypoint.

use agentkit_tackle::acp_v1::{self, TackleState};
use agentkit_tackle::store::SessionStore;
use agentkit_tackle::{cli, config, definitions, telemetry};
use clap::Parser;
use std::process::ExitCode;
use std::sync::Arc;

fn main() -> ExitCode {
    let args = cli::Cli::parse();

    match &args.command {
        cli::Commands::Docgen { kind } => handle_docgen(kind),
        cli::Commands::Auth {
            config_dir,
            credential_helper,
            command,
        } => handle_auth(config_dir.as_ref(), credential_helper.as_deref(), command),
        _ => {
            let _telemetry = telemetry::init();

            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("build tokio runtime");

            runtime.block_on(run(args))
        }
    }
}

async fn run(args: cli::Cli) -> ExitCode {
    let (bind, http_port, db_override, config_override) = match &args.command {
        cli::Commands::Stdio {
            db_path,
            config_dir,
        } => (None, None, db_path.as_ref(), config_dir.as_ref()),
        cli::Commands::Http {
            bind,
            http_port,
            db_path,
            config_dir,
        } => (
            Some(bind.as_str()),
            Some(*http_port),
            db_path.as_ref(),
            config_dir.as_ref(),
        ),
        cli::Commands::Docgen { .. } => unreachable!("handled before the runtime"),
        cli::Commands::Auth { .. } => unreachable!("handled before the runtime"),
    };

    tracing::info!("tackle starting");

    let user_config_dir = cli::config_dir(config_override);
    let db_path = cli::db_path(db_override);
    let project_config_dir = cli::project_config_dir_at(std::path::Path::new("."));
    tracing::info!(db_path = %db_path.display(), "resolved paths");

    let identity = config::trust::project_identity(std::path::Path::new("."));
    let trust_path = user_config_dir.join("trust.toml");
    let trust_store = match config::trust::TrustStore::load(trust_path) {
        Ok(store) => store,
        Err(err) => return fail(err),
    };

    // The project layer is not merged at load: its executable entries are
    // trust-gated, and consent is resolved at the first session (there is
    // no ACP connection at startup to prompt through).
    let (loaded, project_config) =
        match config::load_layered_deferred(&user_config_dir, Some(&project_config_dir)) {
            Ok(loaded) => loaded,
            Err(err) => return fail(err),
        };
    tracing::info!(
        user_layer = ?loaded.user_layer,
        project_layer = ?loaded.project_layer,
        endpoints = loaded.config.endpoints.len(),
        mcp_servers = loaded.config.mcp_servers.len(),
        scripts = loaded.config.scripts.len(),
        "configuration loaded"
    );

    let mut definitions = match definitions::discover(&user_config_dir, Some(&project_config_dir)) {
        Ok(definitions) => definitions,
        Err(err) => return fail(err),
    };
    // Keep the full definitions for the pending project layer, but drop
    // the project entries from the active set until they are trusted.
    let full_definitions = definitions.clone();
    definitions.retain_project(&project_config_dir, &std::collections::BTreeSet::new());
    tracing::info!(
        personas = definitions.personas.len(),
        actors = definitions.actors.len(),
        prompts = definitions.prompts.len(),
        "definitions loaded"
    );

    let db = match SessionStore::connect_sqlite(&db_path).await {
        Ok(db) => db,
        Err(err) => return fail(err),
    };

    // MCP pools are per-connection: each connection registers and
    // connects the configured servers itself, so `/mcp` toggles and
    // client-forwarded servers never bleed between connections.
    let state = Arc::new(TackleState::new(Arc::new(db), loaded, definitions));

    // Hold the project layer until the first session resolves trust. A
    // project may have config, definitions, or both.
    let project_entries = full_definitions.project_trust_entries(&project_config_dir);
    if project_config.is_some() || !project_entries.is_empty() {
        state
            .set_pending_project(acp_v1::PendingProject {
                config: project_config.unwrap_or_default(),
                definitions: full_definitions,
                project_dir: project_config_dir.clone(),
                identity,
                trust_store,
            })
            .await;
    }

    match bind {
        None => match acp_v1::run_stdio(state).await {
            Ok(()) => {
                tracing::info!("shutdown complete");
                ExitCode::SUCCESS
            }
            Err(err) => fail(err),
        },
        Some(bind) => {
            match acp_v1::http::run_http(state.clone(), bind, http_port.unwrap_or(3811)).await {
                Ok(()) => {
                    tracing::info!("shutdown complete");
                    ExitCode::SUCCESS
                }
                Err(err) => fail(err),
            }
        }
    }
}

fn handle_auth(
    config_dir: Option<&std::path::PathBuf>,
    credential_helper: Option<&str>,
    command: &cli::AuthCommands,
) -> ExitCode {
    let user_config_dir = cli::config_dir(config_dir);
    let loaded = match config::load_layered(&user_config_dir, None, None) {
        Ok(loaded) => loaded,
        Err(err) => return fail(err),
    };
    let command = match command {
        cli::AuthCommands::Add { identity, value } => cli::auth::AuthCommand::Add {
            identity: identity.clone(),
            value: value.clone(),
        },
        cli::AuthCommands::Status { identity } => cli::auth::AuthCommand::Status {
            identity: identity.clone(),
        },
        cli::AuthCommands::Token { identity } => cli::auth::AuthCommand::Token {
            identity: identity.clone(),
        },
        cli::AuthCommands::Logout { identity } => cli::auth::AuthCommand::Logout {
            identity: identity.clone(),
        },
    };
    match cli::auth::handle_auth(command, &loaded.config, credential_helper) {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(err) => fail(err),
    }
}

fn handle_docgen(kind: &cli::DocgenCommand) -> ExitCode {
    use clap::CommandFactory as _;
    match kind {
        cli::DocgenCommand::Cli => {
            print!(
                "{}",
                agentkit_docgen::generate_cli_docs(&cli::Cli::command())
            )
        }
    }
    ExitCode::SUCCESS
}

fn fail(err: impl std::fmt::Display) -> ExitCode {
    eprintln!("tackle: {err}");
    ExitCode::FAILURE
}
