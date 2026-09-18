//! Tackle: ACP-native agentic coding harness. Binary entrypoint.

use agentkit_tackle::acp::{self, TackleState};
use agentkit_tackle::store::SessionStore;
use agentkit_tackle::{cli, config, loader, telemetry};
use clap::Parser;
use std::process::ExitCode;
use std::sync::Arc;

fn main() -> ExitCode {
    let args = cli::Cli::parse();
    telemetry::init();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    runtime.block_on(run(args))
}

async fn run(args: cli::Cli) -> ExitCode {
    tracing::info!(transport = ?args.transport, "tackle starting");

    let user_config_dir = cli::config_dir(&args);
    let db_path = cli::db_path(&args);
    let project_config_dir = cli::project_config_dir_at(std::path::Path::new("."));
    tracing::info!(db_path = %db_path.display(), "resolved paths");

    let identity = config::trust::project_identity(std::path::Path::new("."));
    let trust_path = user_config_dir.join("trust.toml");
    let mut trust_store = match config::trust::TrustStore::load(trust_path) {
        Ok(store) => store,
        Err(err) => return fail(err),
    };
    let prompt = config::trust::DenyPrompt;
    let mut gate = config::trust::Gate {
        store: &mut trust_store,
        identity: &identity,
        prompt: &prompt,
    };

    let loaded =
        match config::load_layered(&user_config_dir, Some(&project_config_dir), Some(&mut gate)) {
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

    let mut definitions = match loader::discover(&user_config_dir, Some(&project_config_dir)) {
        Ok(definitions) => definitions,
        Err(err) => return fail(err),
    };
    let project_entries = definitions.project_trust_entries(&project_config_dir);
    let approved = gate.gate(&project_entries);
    definitions.retain_project(&project_config_dir, &approved);
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

    let state = Arc::new(TackleState {
        db,
        config: loaded,
        definitions,
    });

    match args.transport {
        cli::Transport::Stdio => match acp::run_stdio(state).await {
            Ok(()) => {
                tracing::info!("shutdown complete");
                ExitCode::SUCCESS
            }
            Err(err) => fail(err),
        },
        cli::Transport::Http => {
            eprintln!("tackle: HTTP transport is not implemented yet (T-036)");
            ExitCode::FAILURE
        }
    }
}

fn fail(err: impl std::fmt::Display) -> ExitCode {
    eprintln!("tackle: {err}");
    ExitCode::FAILURE
}
