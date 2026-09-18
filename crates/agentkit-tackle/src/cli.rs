//! Command-line interface: transport selection and path overrides.
//!
//! Tackle is a server launched by ACP clients; all session interaction,
//! including resume, is client-driven over ACP — the CLI only selects
//! transports and shutdown behaviour. Path resolution arrives with the
//! layered configuration module (T-003).

use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Transport {
    /// JSON-RPC over stdin/stdout; one process per client connection.
    Stdio,
    /// Streamable HTTP; multiple client connections in one process.
    Http,
}

#[derive(Debug, Parser)]
#[command(name = "tackle", about = "ACP-native agentic coding harness", version)]
pub struct Cli {
    /// Transport to serve the ACP connection over.
    #[arg(long, value_enum, default_value_t = Transport::Stdio)]
    pub transport: Transport,

    /// Network interface to bind in HTTP mode.
    #[arg(long, default_value = "127.0.0.1")]
    pub bind: String,

    /// Port to listen on in HTTP mode.
    #[arg(long, default_value_t = 3811)]
    pub http_port: u16,

    /// Override the session database location (defaults to the platform
    /// data directory).
    #[arg(long)]
    pub db_path: Option<PathBuf>,

    /// Override the user configuration directory (defaults to the platform
    /// config directory).
    #[arg(long)]
    pub config_dir: Option<PathBuf>,
}

/// Waits for a shutdown signal: Ctrl-C, or SIGTERM on Unix.
pub async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = term.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_stdio() {
        let args = Cli::parse_from(["tackle"]);
        assert_eq!(args.transport, Transport::Stdio);
        assert_eq!(args.http_port, 3811);
        assert_eq!(args.bind, "127.0.0.1");
    }
}
