//! Tackle: ACP-native agentic coding harness.

mod acp;
mod agent;
mod builtins;
mod cli;
mod config;
mod invokables;
mod loader;
mod mcp;
mod permissions;
mod scripts;
mod store;
mod telemetry;

use clap::Parser;

fn main() -> std::process::ExitCode {
    let args = cli::Cli::parse();
    telemetry::init();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    runtime.block_on(async move {
        tracing::info!(transport = ?args.transport, "tackle starting");

        let user_config_dir = cli::config_dir(&args);
        let db_path = cli::db_path(&args);
        let project_config_dir = cli::project_config_dir_at(std::path::Path::new("."));
        tracing::info!(db_path = %db_path.display(), "resolved paths");
        let loaded = match config::load_layered(&user_config_dir, Some(&project_config_dir)) {
            Ok(loaded) => loaded,
            Err(err) => {
                eprintln!("tackle: {err}");
                return;
            }
        };
        tracing::info!(
            user_layer = ?loaded.user_layer,
            project_layer = ?loaded.project_layer,
            endpoints = loaded.config.endpoints.len(),
            mcp_servers = loaded.config.mcp_servers.len(),
            scripts = loaded.config.scripts.len(),
            "configuration loaded"
        );

        // The ACP server (T-009) replaces this stub; until then the process
        // idles until shutdown is requested.
        cli::wait_for_shutdown().await;
        tracing::info!("shutdown complete");
    });

    std::process::ExitCode::SUCCESS
}
