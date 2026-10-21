//! Command-line interface: transport subcommands and path overrides.
//!
//! Tackle is a server launched by ACP clients; all session interaction,
//! including resume, is client-driven over ACP — the CLI only selects
//! transports and shutdown behaviour.
//!
//! This module is declarations and re-exports only.

mod args;
mod paths;
mod shutdown;

pub mod auth;
pub mod definitions;

#[cfg(test)]
mod tests;

pub use args::{AuthCommands, Cli, Commands, DefinitionsCommands, DocgenCommand};
pub use paths::{config_dir, db_path, project_config_dir_at};
pub use shutdown::wait_for_shutdown;
