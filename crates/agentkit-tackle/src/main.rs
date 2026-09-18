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
        // The ACP server (T-009) replaces this stub; until then the process
        // idles until shutdown is requested.
        cli::wait_for_shutdown().await;
        tracing::info!("shutdown complete");
    });

    std::process::ExitCode::SUCCESS
}
