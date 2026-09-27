//! Tackle: ACP-native agentic coding harness. Binary entrypoint.

use agentkit_tackle::acp::{self, TackleState};
use agentkit_tackle::mcp::McpPool;
use agentkit_tackle::store::SessionStore;
use agentkit_tackle::{cli, config, loader, telemetry};
use clap::Parser;
use std::process::ExitCode;
use std::sync::Arc;

fn main() -> ExitCode {
    let args = cli::Cli::parse();

    match &args.command {
        cli::Commands::Docgen { kind } => handle_docgen(kind),
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
        cli::Commands::Stdio { db_path, config_dir } => (None, None, db_path.as_ref(), config_dir.as_ref()),
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
    };

    tracing::info!("tackle starting");

    let user_config_dir = cli::config_dir(config_override);
    let db_path = cli::db_path(db_override);
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

    // Every configured server is registered, then connected
    // concurrently: a dead server must not hold up the rest, and a
    // failure is a status rather than a startup error. Connections are
    // made on throwaway single-server pools — `connect` needs unique
    // access — and folded back in. Each is registered enabled, so
    // `/mcp disable` is the only thing that takes one offline.
    let helper = agentkit_tackle::agent::provider::credential_helper_name(&loaded.config);
    let new_pool = || McpPool::new().with_credential_helper(helper.clone());
    let mut mcp_pool = new_pool();
    for (server_name, server_config) in &loaded.config.mcp_servers {
        mcp_pool.register(server_name, server_config.clone());
    }
    let connecting = loaded
        .config
        .mcp_servers
        .iter()
        .map(|(server_name, server_config)| {
            let server_name = server_name.clone();
            let server_config = server_config.clone();
            let mut one = new_pool();
            one.register(&server_name, server_config.clone());
            async move {
                one.connect(&server_name, &server_config).await;
                one
            }
        })
        .collect::<Vec<_>>();
    for connected in futures::future::join_all(connecting).await {
        mcp_pool.absorb(connected);
    }
    tracing::info!(servers = mcp_pool.server_names().len(), "mcp pool ready");
    let state = Arc::new(TackleState {
        db: Arc::new(db),
        config: loaded,
        definitions,
        mcp_pool: tokio::sync::Mutex::new(mcp_pool),
    });

    match bind {
        None => match acp::run_stdio(state).await {
            Ok(()) => {
                tracing::info!("shutdown complete");
                ExitCode::SUCCESS
            }
            Err(err) => fail(err),
        },
        Some(bind) => match acp::http::run_http(state.clone(), bind, http_port.unwrap_or(3811)).await
        {
            Ok(()) => {
                tracing::info!("shutdown complete");
                ExitCode::SUCCESS
            }
            Err(err) => fail(err),
        },
    }
}

fn handle_docgen(kind: &cli::DocgenCommand) -> ExitCode {
    use clap::CommandFactory as _;
    match kind {
        cli::DocgenCommand::Cli => {
            print!("{}", agentkit_docgen::generate_cli_docs(&cli::Cli::command()))
        }
    }
    ExitCode::SUCCESS
}

fn fail(err: impl std::fmt::Display) -> ExitCode {
    eprintln!("tackle: {err}");
    ExitCode::FAILURE
}
