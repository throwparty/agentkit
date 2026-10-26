//! The clap argument surface: transport subcommands, path overrides, and the auth/docgen commands.

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

        /// Bearer token required on `/rpc` and `/events`. Required when
        /// binding a non-loopback interface.
        #[arg(long)]
        http_token: Option<String>,

        /// Override the session database location (defaults to the platform
        /// data directory).
        #[arg(long)]
        db_path: Option<PathBuf>,

        /// Override the user configuration directory (defaults to the
        /// platform config directory).
        #[arg(long)]
        config_dir: Option<PathBuf>,
    },
    /// Manage stored credentials
    ///
    /// Store, inspect, and remove the bearer tokens Tackle resolves
    /// through the credential helper.
    Auth {
        /// Override the user configuration directory (defaults to the platform
        /// config directory).
        #[arg(long)]
        config_dir: Option<PathBuf>,

        /// Override the credential helper name (defaults to the configured
        /// helper, or `keychain`).
        #[arg(long)]
        credential_helper: Option<String>,

        #[command(subcommand)]
        command: AuthCommands,
    },
    /// Manage definition sources, selections, and vendoring
    ///
    /// `def` is an alias.
    #[command(alias = "def")]
    Definitions {
        /// Repository root (defaults to the current directory).
        #[arg(long, default_value = ".")]
        project_dir: PathBuf,

        #[command(subcommand)]
        command: DefinitionsCommands,
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
pub enum AuthCommands {
    /// Store a bearer token for an identity
    Add { identity: String, value: String },
    /// Report configured identities and their credential sources
    Status { identity: Option<String> },
    /// Print the resolved credential for an identity
    Token { identity: String },
    /// Remove a stored credential
    Logout { identity: String },
}

#[derive(Debug, Subcommand)]
pub enum DefinitionsCommands {
    /// Scaffold a definitions manifest
    Init,
    /// Select a definition from a declared source
    Add {
        source: String,
        kind: String,
        name: String,
    },
    /// Remove a selected definition
    Remove {
        source: String,
        kind: String,
        name: String,
    },
    /// Vendor the locked revisions
    Get,
    /// Re-resolve refs, re-vendor, and rewrite the lock
    Update,
    /// Report declared and vendored definitions
    List,
    /// Check vendored packages against the lock
    Verify,
}

#[derive(Debug, Subcommand)]
pub enum DocgenCommand {
    /// Generate CLI reference documentation
    Cli,
}
