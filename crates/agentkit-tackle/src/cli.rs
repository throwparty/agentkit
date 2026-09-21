//! Command-line interface: transport subcommands and path overrides.
//!
//! Tackle is a server launched by ACP clients; all session interaction,
//! including resume, is client-driven over ACP — the CLI only selects
//! transports and shutdown behaviour.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "tackle", about = "ACP-native agentic coding harness", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Serve ACP over JSON-RPC on stdin/stdout
    ///
    /// One process per client connection.
    Stdio {
        /// Override the session database location (defaults to the platform
        /// data directory).
        #[arg(long)]
        db_path: Option<PathBuf>,

        /// Override the user configuration directory (defaults to the
        /// platform config directory).
        #[arg(long)]
        config_dir: Option<PathBuf>,
    },
    /// Serve the streamable HTTP transport
    ///
    /// Multiple client connections in one process.
    Http {
        /// Network interface to bind.
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,

        /// Port to listen on.
        #[arg(long, default_value_t = 3811)]
        http_port: u16,

        /// Override the session database location (defaults to the platform
        /// data directory).
        #[arg(long)]
        db_path: Option<PathBuf>,

        /// Override the user configuration directory (defaults to the
        /// platform config directory).
        #[arg(long)]
        config_dir: Option<PathBuf>,
    },
    /// Generate reference documentation
    ///
    /// Prints docs to stdout.
    #[command(hide = true)]
    Docgen {
        #[command(subcommand)]
        kind: DocgenCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum DocgenCommand {
    /// Generate CLI reference documentation
    Cli,
}

/// Resolves the effective user configuration directory, honouring the
/// subcommand override.
pub fn config_dir(config_dir: Option<&PathBuf>) -> PathBuf {
    config_dir
        .cloned()
        .unwrap_or_else(|| agentkit_path::config_dir("tackle"))
}

/// Resolves the effective session database path, honouring the subcommand
/// override.
pub fn db_path(db_path: Option<&PathBuf>) -> PathBuf {
    db_path
        .cloned()
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
    use clap::CommandFactory;

    #[test]
    fn parses_stdio() {
        let args = Cli::parse_from(["tackle", "stdio"]);
        assert!(matches!(args.command, Commands::Stdio { .. }));
    }

    #[test]
    fn parses_http_defaults() {
        let args = Cli::parse_from(["tackle", "http"]);
        match args.command {
            Commands::Http {
                bind, http_port, ..
            } => {
                assert_eq!(bind, "127.0.0.1");
                assert_eq!(http_port, 3811);
            }
            _ => panic!("expected http"),
        }
    }

    #[test]
    fn resolves_defaults() {
        let args = Cli::parse_from(["tackle", "stdio"]);
        let (db, cfg) = match &args.command {
            Commands::Stdio { db_path, config_dir } => (db_path.as_ref(), config_dir.as_ref()),
            _ => panic!("expected stdio"),
        };
        assert!(db_path(db).parent().unwrap().ends_with("tackle"));
        assert!(config_dir(cfg).ends_with("tackle"));
    }

    #[test]
    fn honours_overrides() {
        let args = Cli::parse_from([
            "tackle",
            "http",
            "--config-dir",
            "/tmp/tackle-cfg",
            "--db-path",
            "/tmp/tackle.db",
        ]);
        match &args.command {
            Commands::Http {
                db_path: db,
                config_dir: cfg,
                ..
            } => {
                assert_eq!(config_dir(cfg.as_ref()), PathBuf::from("/tmp/tackle-cfg"));
                assert_eq!(db_path(db.as_ref()), PathBuf::from("/tmp/tackle.db"));
            }
            _ => panic!("expected http"),
        }
    }

    #[test]
    fn command_builds() {
        Cli::command().build();
    }
}
