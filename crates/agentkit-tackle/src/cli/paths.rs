//! Path resolution honouring the subcommand overrides.

use std::path::PathBuf;

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
