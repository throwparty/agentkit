//! Configuration shared by the CLI and the MCP server.
//!
//! Two scopes: the person running the code owns the mechanism — its selection,
//! its settings and where forwarded ports bind — and the project owns what the
//! project needs, such as its setup command and the ports it expects. A
//! mechanism's keys are never typed here; they stay untyped until the backend
//! that owns them parses its own section.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::domain::ScmMode;

/// A port the project expects to be forwarded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortRequest {
    /// Stable name, used to derive the environment variable.
    pub name: String,
    /// Port inside the sandbox.
    pub target: u16,
}

/// How forwarded ports are exposed on the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardingConfig {
    /// The host address forwarded ports bind to.
    pub bind: String,
}

impl Default for ForwardingConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1".to_string(),
        }
    }
}

/// What the project declares about itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    /// Overrides the project slug derived from the directory name.
    pub slug: Option<String>,
    /// A command run once, confined, after the workspace is materialised.
    #[serde(rename = "setup-command")]
    pub setup_command: Option<String>,
}

/// How sandboxes are made from the repository.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitConfig {
    /// Whether snapshots go to a branch or a bare clone.
    #[serde(rename = "snapshot-mode", default)]
    pub snapshot_mode: Option<ScmMode>,
}

/// The merged configuration.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    /// The backend the user selected.
    pub backend: Option<String>,
    /// One untyped section per backend, keyed by backend name.
    pub backends: BTreeMap<String, toml::Value>,
    /// What the project declares.
    pub project: ProjectConfig,
    /// Ports the project expects.
    pub ports: Vec<PortRequest>,
    /// How forwarded ports are exposed.
    pub forwarding: ForwardingConfig,
    /// Whether a degraded mechanism may be used.
    pub accept_degraded: bool,
    /// Whether any repository may run its own setup command.
    pub allow_project_setup: bool,
    /// Projects allowed to run their own setup command, by namespace.
    pub allowed_projects: Vec<String>,
    /// How sandboxes are made.
    pub git: GitConfig,
    /// Settings that were accepted but ignored, and why.
    pub notices: Vec<String>,
}

/// A configuration file. The committed project file and the untracked local
/// override share a shape; only the local file may choose or configure a
/// backend, and only it may grant setup consent.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectFile {
    /// What the project declares.
    #[serde(default)]
    pub project: ProjectConfig,
    /// Ports the project expects.
    #[serde(default)]
    pub ports: Vec<PortRequest>,
    /// How sandboxes are made.
    #[serde(default)]
    pub git: GitConfig,
    /// The backend the person running the code selected.
    pub backend: Option<String>,
    /// One untyped section per backend.
    #[serde(default)]
    pub backends: BTreeMap<String, toml::Value>,
    /// How forwarded ports are exposed on this machine.
    pub forwarding: Option<ForwardingConfig>,
    /// Accept a degraded mechanism rather than refusing.
    #[serde(default, rename = "accept-degraded")]
    pub accept_degraded: bool,
    /// Allow any repository to run its own setup command.
    #[serde(default, rename = "allow-project-setup")]
    pub allow_project_setup: bool,
    /// Allow these projects, by namespace, to run their setup command.
    #[serde(default, rename = "allowed-projects")]
    pub allowed_projects: Vec<String>,
}

/// Merge the local override with the committed project file, reporting anything
/// the project tried to decide that belongs to the machine.
pub fn merge(local: ProjectFile, project: ProjectFile) -> Config {
    let mut notices = Vec::new();

    if project.backend.is_some() {
        notices.push(
            "project configuration names a backend; the backend is chosen by the person \
             running the code, so the setting is ignored"
                .to_string(),
        );
    }
    if !project.backends.is_empty() {
        notices.push(
            "project configuration configures a backend; backend settings belong to the \
             machine, so they are ignored"
                .to_string(),
        );
    }
    if project.forwarding.is_some() {
        notices.push(
            "project configuration sets where forwarded ports bind; that is the machine's \
             decision, so it is ignored"
                .to_string(),
        );
    }
    if project.accept_degraded {
        notices.push(
            "project configuration accepts degraded mechanisms; that is the machine's \
             decision, so it is ignored"
                .to_string(),
        );
    }
    if project.allow_project_setup || !project.allowed_projects.is_empty() {
        notices.push(
            "project configuration sets setup consent; that is the machine's decision, so \
             it is ignored"
                .to_string(),
        );
    }

    Config {
        backend: local.backend,
        backends: local.backends,
        project: ProjectConfig {
            slug: local.project.slug.or(project.project.slug),
            setup_command: local
                .project
                .setup_command
                .or(project.project.setup_command),
        },
        ports: if local.ports.is_empty() {
            project.ports
        } else {
            local.ports
        },
        forwarding: local.forwarding.unwrap_or_default(),
        accept_degraded: local.accept_degraded,
        allow_project_setup: local.allow_project_setup,
        allowed_projects: local.allowed_projects,
        git: GitConfig {
            snapshot_mode: local.git.snapshot_mode.or(project.git.snapshot_mode),
        },
        notices,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("File not found: {0}")]
    FileNotFound(std::path::PathBuf),
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Could not read {path}: {detail}")]
    Io {
        /// The file that could not be read.
        path: std::path::PathBuf,
        /// The underlying reason.
        detail: String,
    },
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{ConfigError, ForwardingConfig, PortRequest, ProjectConfig, ProjectFile, merge};

    fn local() -> ProjectFile {
        ProjectFile {
            backend: Some("docker".to_string()),
            backends: BTreeMap::from([(
                "docker".to_string(),
                toml::Value::Table(Default::default()),
            )]),
            forwarding: Some(ForwardingConfig {
                bind: "127.0.0.1".to_string(),
            }),
            ..ProjectFile::default()
        }
    }

    fn project() -> ProjectFile {
        ProjectFile {
            project: ProjectConfig {
                slug: Some("project".to_string()),
                setup_command: Some("echo project".to_string()),
            },
            ports: vec![PortRequest {
                name: "web".to_string(),
                target: 8080,
            }],
            ..ProjectFile::default()
        }
    }

    #[test]
    fn the_local_file_chooses_the_mechanism_and_the_project_supplies_its_needs() {
        let config = merge(local(), project());
        assert_eq!(config.backend.as_deref(), Some("docker"));
        assert!(config.backends.contains_key("docker"));
        assert_eq!(
            config.project.setup_command.as_deref(),
            Some("echo project")
        );
        assert_eq!(config.ports.len(), 1);
        assert_eq!(config.forwarding.bind, "127.0.0.1");
        assert!(config.notices.is_empty());
    }

    #[test]
    fn a_project_naming_a_backend_is_reported_and_ignored() {
        let mut project = project();
        project.backend = Some("bubblewrap".to_string());
        let config = merge(local(), project);
        assert_eq!(config.backend.as_deref(), Some("docker"));
        assert!(
            config
                .notices
                .iter()
                .any(|notice| notice.contains("names a backend")),
            "{:?}",
            config.notices
        );
    }

    #[test]
    fn a_project_configuring_a_backend_is_reported_and_ignored() {
        let mut project = project();
        project
            .backends
            .insert("docker".to_string(), toml::Value::Table(Default::default()));
        let config = merge(local(), project);
        assert_eq!(config.backends.len(), 1);
        assert!(
            config
                .notices
                .iter()
                .any(|notice| notice.contains("configures a backend")),
            "{:?}",
            config.notices
        );
    }

    #[test]
    fn a_project_setting_the_bind_address_is_reported_and_ignored() {
        let mut project = project();
        project.forwarding = Some(ForwardingConfig {
            bind: "0.0.0.0".to_string(),
        });
        let config = merge(local(), project);
        assert_eq!(config.forwarding.bind, "127.0.0.1");
        assert!(
            config.notices.iter().any(|notice| notice.contains("bind")),
            "{:?}",
            config.notices
        );
    }

    #[test]
    fn the_local_file_selects_the_machine_settings_and_overrides_ports() {
        let local = ProjectFile {
            backend: Some("bubblewrap".to_string()),
            backends: BTreeMap::from([(
                "docker".to_string(),
                toml::Value::Table(Default::default()),
            )]),
            forwarding: Some(ForwardingConfig {
                bind: "0.0.0.0".to_string(),
            }),
            ports: vec![PortRequest {
                name: "local-web".to_string(),
                target: 9090,
            }],
            ..ProjectFile::default()
        };
        let config = merge(local, project());

        assert_eq!(config.backend.as_deref(), Some("bubblewrap"));
        assert_eq!(config.forwarding.bind, "0.0.0.0");
        assert_eq!(config.ports.len(), 1);
        assert_eq!(config.ports[0].name, "local-web");
        assert_eq!(config.ports[0].target, 9090);
        assert!(config.notices.is_empty(), "{:?}", config.notices);
    }

    #[test]
    fn the_local_file_overrides_the_project_file() {
        let local = ProjectFile {
            project: ProjectConfig {
                setup_command: Some("echo local".to_string()),
                slug: None,
            },
            ..ProjectFile::default()
        };
        let config = merge(local, project());
        assert_eq!(config.project.setup_command.as_deref(), Some("echo local"));
        assert_eq!(config.project.slug.as_deref(), Some("project"));
    }

    #[test]
    fn forwarding_defaults_to_loopback() {
        let config = merge(ProjectFile::default(), project());
        assert_eq!(config.forwarding.bind, "127.0.0.1");
    }

    #[test]
    fn a_missing_file_is_distinguishable() {
        let error = ConfigError::FileNotFound("/nowhere".into());
        assert!(error.to_string().contains("/nowhere"));
    }
}
