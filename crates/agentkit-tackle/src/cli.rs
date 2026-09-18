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

/// Resolves the effective user configuration directory, honouring the
/// CLI override.
pub fn config_dir(args: &Cli) -> PathBuf {
    args.config_dir
        .clone()
        .unwrap_or_else(|| agentkit_path::config_dir("tackle"))
}

/// Resolves the effective session database path, honouring the CLI override.
pub fn db_path(args: &Cli) -> PathBuf {
    args.db_path
        .clone()
        .unwrap_or_else(|| agentkit_path::data_dir("tackle").join("sessions.db"))
}

/// Resolves the effective project configuration directory anchored at an
/// explicit working directory (used when the process cwd differs from the
/// session cwd).
pub fn project_config_dir_at(cwd: &std::path::Path) -> PathBuf {
    cwd.join(".agentkit").join("tackle")
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

    #[test]
    fn config_dir_defaults_to_platform_path() {
        let args = Cli::parse_from(["tackle"]);
        let path = config_dir(&args);
        assert!(path.is_absolute());
        assert!(path.ends_with("tackle"));
    }

    #[test]
    fn config_dir_override_is_honoured() {
        let args = Cli::parse_from(["tackle", "--config-dir", "/tmp/tackle-cfg"]);
        assert_eq!(config_dir(&args), PathBuf::from("/tmp/tackle-cfg"));
    }

    #[test]
    fn db_path_defaults_to_data_dir() {
        let args = Cli::parse_from(["tackle"]);
        let path = db_path(&args);
        assert!(path.ends_with("sessions.db"));
        assert!(path.is_absolute());
    }

    #[test]
    fn db_path_override_is_honoured() {
        let args = Cli::parse_from(["tackle", "--db-path", "/tmp/tackle.db"]);
        assert_eq!(db_path(&args), PathBuf::from("/tmp/tackle.db"));
    }

    #[test]
    fn project_config_dir_anchors_at_cwd() {
        let dir = project_config_dir_at(std::path::Path::new("/work"));
        assert_eq!(dir, PathBuf::from("/work/.agentkit/tackle"));
    }
}
