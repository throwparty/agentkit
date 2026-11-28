//! Git's configuration section, typed here and nowhere else.
//!
//! The core configuration holds this section as an untyped value; only this
//! module knows which keys Git accepts, and a key it does not accept is an
//! error rather than something silently ignored.

use serde::Deserialize;
use std::path::Path;
use std::process::Command;

use crate::domain::{SandboxError, ScmMode};
use crate::sandbox::contract::WorkspaceBinding;

/// Git's keys.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GitVcsConfig {
    /// Whether snapshots go to a branch or a bare clone.
    #[serde(rename = "snapshot-mode", default)]
    pub snapshot_mode: Option<ScmMode>,
    /// How a sandbox receives its workspace.
    #[serde(default)]
    pub binding: Option<WorkspaceBinding>,
    /// Configuration applied to Litterbox's clone. Git config syntax.
    #[serde(default)]
    pub config: Option<String>,
}

impl GitVcsConfig {
    /// Parse the section from the core's untyped map.
    pub fn from_section(section: &toml::Value) -> Result<Self, String> {
        if !section.is_table() {
            return Err("vcs.git must be a table".to_string());
        }
        section
            .clone()
            .try_into()
            .map_err(|error: toml::de::Error| format!("vcs.git is not valid: {error}"))
    }
}

/// Apply a Git config string to Litterbox's clone.
///
/// The string is written to an overlay file the clone includes. The overlay is
/// replaced on every call, so an empty or absent string clears the previous
/// settings. Git validates the string before it is installed.
pub fn apply_config(store: &Path, config: Option<&str>) -> Result<(), SandboxError> {
    // Project opens can race. The candidate and overlay are shared paths, so
    // hold one interprocess lock across write, validation, replacement and
    // installation.
    let lock_path = store.with_extension("litterbox.config.lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(SandboxError::Io)?;
    lock.lock().map_err(SandboxError::Io)?;

    let overlay = store.join("litterbox.config");
    let temporary = store.join("litterbox.config.tmp");
    std::fs::write(&temporary, config.unwrap_or("")).map_err(SandboxError::Io)?;

    let checked = Command::new("git")
        .arg("config")
        .arg("--file")
        .arg(&temporary)
        .arg("--list")
        .output()
        .map_err(SandboxError::Io)?;
    if !checked.status.success() {
        let _ = std::fs::remove_file(&temporary);
        return Err(SandboxError::Config(format!(
            "vcs.git config is not valid: {}",
            String::from_utf8_lossy(&checked.stderr).trim()
        )));
    }

    std::fs::rename(&temporary, &overlay).map_err(SandboxError::Io)?;
    let overlay = overlay.canonicalize().map_err(SandboxError::Io)?;
    let installed = Command::new("git")
        .arg("--git-dir")
        .arg(store)
        .args(["config", "--replace-all", "include.path"])
        .arg(&overlay)
        .output()
        .map_err(SandboxError::Io)?;
    if !installed.status.success() {
        return Err(SandboxError::Config(format!(
            "vcs.git config could not be installed: {}",
            String::from_utf8_lossy(&installed.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use tempfile::TempDir;

    use super::GitVcsConfig;
    use crate::domain::ScmMode;
    use crate::sandbox::contract::WorkspaceBinding;

    fn section(text: &str) -> toml::Value {
        toml::from_str(text).expect("section")
    }

    fn bare() -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        git2::Repository::init_bare(dir.path()).expect("bare");
        dir
    }

    fn read(dir: &Path, key: &str) -> Option<String> {
        let output = Command::new("git")
            .arg("--git-dir")
            .arg(dir)
            .args(["config", "--get", key])
            .output()
            .expect("git runs");
        if output.status.success() {
            Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            None
        }
    }

    #[test]
    fn the_config_string_parses() {
        let config = GitVcsConfig::from_section(&section("config = 'x'")).expect("config");
        assert_eq!(config.config.as_deref(), Some("x"));
    }

    #[test]
    fn a_config_string_is_applied_and_replaced() {
        let dir = bare();
        super::apply_config(dir.path(), Some("[gc]\n\treflogExpire = 30.days\n")).expect("apply");
        assert_eq!(
            read(dir.path(), "gc.reflogExpire").as_deref(),
            Some("30.days")
        );

        super::apply_config(
            dir.path(),
            Some("[gc]\n\treflogExpireUnreachable = 7.days\n"),
        )
        .expect("replace");
        assert_eq!(read(dir.path(), "gc.reflogExpire"), None);
        assert_eq!(
            read(dir.path(), "gc.reflogExpireUnreachable").as_deref(),
            Some("7.days")
        );

        super::apply_config(dir.path(), None).expect("clear");
        assert_eq!(read(dir.path(), "gc.reflogExpireUnreachable"), None);
    }

    #[test]
    fn an_invalid_config_is_refused() {
        let dir = bare();
        let error = super::apply_config(dir.path(), Some("not a config")).expect_err("invalid");
        assert!(error.to_string().contains("vcs.git config"), "{error}");
    }

    #[test]
    fn the_snapshot_mode_parses() {
        let config =
            GitVcsConfig::from_section(&section("snapshot-mode = 'direct'")).expect("config");
        assert_eq!(config.snapshot_mode, Some(ScmMode::Direct));
    }

    #[test]
    fn the_binding_parses_and_an_absent_one_is_none() {
        let config = GitVcsConfig::from_section(&section("binding = 'bound'")).expect("config");
        assert_eq!(config.binding, Some(WorkspaceBinding::Bound));

        let config = GitVcsConfig::from_section(&section("binding = 'copied'")).expect("config");
        assert_eq!(config.binding, Some(WorkspaceBinding::Copied));

        let config =
            GitVcsConfig::from_section(&toml::Value::Table(Default::default())).expect("config");
        assert_eq!(config.binding, None, "an absent binding is not a default");
    }

    #[test]
    fn an_unknown_binding_is_refused_naming_the_values() {
        let error =
            GitVcsConfig::from_section(&section("binding = 'hardlink'")).expect_err("unknown");
        assert!(error.contains("vcs.git"), "{error}");
        assert!(
            error.contains("copied") && error.contains("bound"),
            "{error}"
        );
    }

    #[test]
    fn an_absent_mode_stays_unset() {
        let config =
            GitVcsConfig::from_section(&toml::Value::Table(Default::default())).expect("config");
        assert_eq!(config.snapshot_mode, None);
    }

    #[test]
    fn an_unknown_key_is_refused() {
        let error =
            GitVcsConfig::from_section(&section("image = 'busybox'")).expect_err("unknown key");
        assert!(error.contains("vcs.git"), "{error}");
    }
}
