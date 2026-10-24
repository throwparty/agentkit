//! A deterministic, offline OpenAI-compatible model endpoint used by the
//! acp-tck conformance runner. It is a test fixture, not a product binary:
//! it listens on loopback, derives every response from the request body,
//! and never reaches the network.

use clap::Parser;
use std::net::IpAddr;
use std::process::ExitCode;

#[derive(Debug, Parser)]
struct Args {
    /// Address to bind. Only loopback addresses are accepted.
    #[arg(long, default_value = "127.0.0.1")]
    bind: IpAddr,

    /// TCP port to listen on.
    #[arg(long)]
    port: u16,

    /// Request value that holds the response stream open until cancelled.
    #[arg(long)]
    cancel_sentinel: Option<String>,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    if !args.bind.is_loopback() {
        eprintln!(
            "tackle-test-model: refusing to bind non-loopback address {}",
            args.bind
        );
        return ExitCode::FAILURE;
    }

    if let Some(sentinel) = &args.cancel_sentinel {
        tracing::info!(sentinel = %sentinel, "cancel sentinel configured");
    }

    run(args.bind, args.port).await
}

async fn run(bind: IpAddr, port: u16) -> ExitCode {
    let app = axum::Router::new().route(
        "/healthz",
        axum::routing::get(|| async { axum::http::StatusCode::OK }),
    );

    let listener = match tokio::net::TcpListener::bind((bind, port)).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("tackle-test-model: cannot bind {bind}:{port}: {err}");
            return ExitCode::FAILURE;
        }
    };
    let address = listener
        .local_addr()
        .map(|address| address.to_string())
        .unwrap_or_else(|_| format!("{bind}:{port}"));
    tracing::info!(address = %address, "tackle-test-model listening");

    match axum::serve(listener, app)
        .with_graceful_shutdown(agentkit_tackle::cli::wait_for_shutdown())
        .await
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("tackle-test-model: {err}");
            ExitCode::FAILURE
        }
    }
}
