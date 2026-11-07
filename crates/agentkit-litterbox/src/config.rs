//! Configuration shared by the CLI and the MCP server.
//!
//! Two scopes: the person running the code owns the mechanism — its selection,
//! its settings and where forwarded ports bind — and the project owns what the
//! project needs, such as its provisioning steps, its supported backends and
//! the ports it expects. A mechanism's keys are never typed here; they stay
//! untyped until the backend that owns them parses its own section.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::domain::ScmMode;
use crate::sandbox::contract::{ProvisionStep, StoreView};

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
    /// The backends this project's provisioning supports.
    #[serde(default)]
    pub backends: Option<Vec<String>>,
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
    /// Machine-owned backend settings, keyed by backend name.
    pub backend_settings: BTreeMap<String, toml::Value>,
    /// What the project declares.
    pub project: ProjectConfig,
    /// The step definitions, by identity. This is not a backend's sequence.
    pub provision_definitions: Vec<ProvisionStep>,
    /// Each backend's ordered step identities. This is the sequence, not the
    /// definitions.
    pub backend_sequences: BTreeMap<String, Vec<String>>,
    /// Ports the project expects.
    pub ports: Vec<PortRequest>,
    /// How forwarded ports are exposed.
    pub forwarding: ForwardingConfig,
    /// Which store view a devshell step binds.
    pub store_view: StoreView,
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

impl Config {
    /// Whether this project's command steps may run.
    pub fn command_steps_allowed(&self, project_slug: &str) -> bool {
        self.allow_project_setup
            || self
                .allowed_projects
                .iter()
                .any(|project| project == project_slug)
    }

    /// Check the provisioning schema without changing the configuration.
    ///
    /// This is the one definition of the rules; the loader turns an issue into
    /// a configuration error and the resolver turns the same issue into a
    /// structured provisioning failure.
    pub fn check_provisioning(&self) -> Result<(), ProvisioningIssue> {
        if self.provision_definitions.len() > MAX_PROVISION_STEPS {
            return Err(ProvisioningIssue {
                backend: None,
                step: None,
                reason: format!(
                    "the project declares {} provisioning steps; the limit is \
                     {MAX_PROVISION_STEPS}",
                    self.provision_definitions.len()
                ),
                remedy: "remove steps the project does not need".to_string(),
            });
        }

        let mut identities = HashSet::new();

        for definition in &self.provision_definitions {
            let identity = definition.identity();
            if !valid_identity(identity) {
                let identity = escaped(identity);
                return Err(ProvisioningIssue {
                    backend: None,
                    step: Some(identity.clone()),
                    reason: format!(
                        "provisioning identity '{identity}' must start with a lowercase letter \
                         and contain only lowercase letters, digits, hyphens and underscores"
                    ),
                    remedy: "rename the step to start with a lowercase letter and use only \
                             lowercase letters, digits, hyphens and underscores"
                        .to_string(),
                });
            }
            if !identities.insert(identity.to_string()) {
                return Err(ProvisioningIssue {
                    backend: None,
                    step: Some(identity.to_string()),
                    reason: format!("duplicate provisioning identity '{identity}'"),
                    remedy: "give each provisioning step a unique identity".to_string(),
                });
            }
        }

        for backend in self.project.backends.iter().flatten() {
            if !valid_backend_name(backend) {
                let backend = escaped(backend);
                return Err(ProvisioningIssue {
                    backend: Some(backend.clone()),
                    step: None,
                    reason: format!(
                        "[project] backends names '{backend}', which is not a backend name"
                    ),
                    remedy: "name a backend using letters, digits, hyphens, underscores or dots"
                        .to_string(),
                });
            }
        }

        for (backend, sequence) in &self.backend_sequences {
            if !valid_backend_name(backend) {
                let backend = escaped(backend);
                return Err(ProvisioningIssue {
                    backend: Some(backend.clone()),
                    step: None,
                    reason: format!(
                        "backends.{backend}.provision is keyed by a value that is not a backend \
                         name"
                    ),
                    remedy: "name the backend using letters, digits, hyphens, underscores or dots"
                        .to_string(),
                });
            }
            if sequence.len() > MAX_SEQUENCE_STEPS {
                let display = escaped(backend);
                return Err(ProvisioningIssue {
                    backend: Some(display.clone()),
                    step: None,
                    reason: format!(
                        "backends.{display}.provision lists {} steps; the limit is \
                         {MAX_SEQUENCE_STEPS}",
                        sequence.len()
                    ),
                    remedy: "remove steps from the sequence".to_string(),
                });
            }
            let mut seen = HashSet::new();
            for identity in sequence {
                if !seen.insert(identity) {
                    let identity = escaped(identity);
                    let backend = escaped(backend);
                    return Err(ProvisioningIssue {
                        backend: Some(backend.clone()),
                        step: Some(identity.clone()),
                        reason: format!("backends.{backend}.provision repeats '{identity}'"),
                        remedy: "list each step identity once per backend".to_string(),
                    });
                }
                if !identities.contains(identity) {
                    let identity = escaped(identity);
                    let backend = escaped(backend);
                    return Err(ProvisioningIssue {
                        backend: Some(backend.clone()),
                        step: Some(identity.clone()),
                        reason: format!(
                            "backends.{backend}.provision names '{identity}', which \
                             [[provision]] does not define"
                        ),
                        remedy: "define the step in [[provision]] or remove it from the \
                                 sequence"
                            .to_string(),
                    });
                }
            }
        }

        if !self.provision_definitions.is_empty() {
            match self.project.backends.as_deref() {
                None => {
                    return Err(ProvisioningIssue {
                        backend: None,
                        step: None,
                        reason: "[[provision]] defines steps, so [project] backends must name \
                                 the backends they support"
                            .to_string(),
                        remedy: "add [project] backends naming the supported backends".to_string(),
                    });
                }
                Some([]) => {
                    return Err(ProvisioningIssue {
                        backend: None,
                        step: None,
                        reason: "[project] backends is empty; name at least one backend when \
                                 [[provision]] defines steps"
                            .to_string(),
                        remedy: "name at least one backend in [project] backends".to_string(),
                    });
                }
                Some(_) => {}
            }
        }

        Ok(())
    }

    /// Check the schema and record what the project declared but no backend
    /// uses.
    pub fn validate_provisioning(&mut self) -> Result<(), ProvisioningIssue> {
        self.check_provisioning()?;

        let supported: HashSet<&str> = self
            .project
            .backends
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(String::as_str)
            .collect();
        let used: HashSet<&str> = self
            .backend_sequences
            .values()
            .flatten()
            .map(String::as_str)
            .collect();

        let mut notices = Vec::new();
        for definition in &self.provision_definitions {
            if !used.contains(definition.identity()) {
                notices.push(format!(
                    "provisioning step '{}' is not used by any backend's sequence",
                    definition.identity()
                ));
            }
        }
        for backend in self.backend_sequences.keys() {
            if !supported.contains(backend.as_str()) {
                let display = escaped(backend);
                notices.push(format!(
                    "backends.{display}.provision is ignored because [project] backends does \
                     not list '{display}'"
                ));
            }
        }
        for notice in notices {
            if !self.notices.contains(&notice) {
                self.notices.push(notice);
            }
        }

        Ok(())
    }
}

/// The most step definitions a project may declare.
const MAX_PROVISION_STEPS: usize = 32;

/// The most steps one backend's sequence may list.
const MAX_SEQUENCE_STEPS: usize = 32;

/// A provisioning schema problem, independent of the surface reporting it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvisioningIssue {
    /// The backend the problem belongs to, when one is involved.
    pub backend: Option<String>,
    /// The step identity the problem names, when one is involved.
    pub step: Option<String>,
    /// What is wrong.
    pub reason: String,
    /// What the user can do about it.
    pub remedy: String,
}

impl std::fmt::Display for ProvisioningIssue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Every surface reports the remedy too: the CLI and doctor flatten
        // this into a plain message, so dropping it here loses it for them.
        write!(formatter, "{}; {}", self.reason, self.remedy)
    }
}

/// A provisioning identity: lowercase, hyphenated or underscored, starting
/// with a letter.
fn valid_identity(identity: &str) -> bool {
    let mut chars = identity.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Escape a configuration-supplied value for use in a human-readable message.
pub(crate) fn escaped(value: &str) -> String {
    value.chars().flat_map(char::escape_default).collect()
}

/// A backend name that cannot forge a log line or inject a control sequence.
pub fn valid_backend_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphanumeric())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
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
    /// The provisioning steps the project declares.
    #[serde(default)]
    pub provision: Option<Vec<ProvisionStep>>,
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
    /// Which store view a devshell step uses on this machine.
    #[serde(default, rename = "store-view")]
    pub store_view: Option<StoreView>,
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
pub fn merge(local: ProjectFile, project: ProjectFile) -> Result<Config, ConfigError> {
    let mut notices = Vec::new();

    if project.backend.is_some() {
        notices.push(
            "project configuration names a backend; the backend is chosen by the person \
             running the code, so the setting is ignored"
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
    if project.store_view.is_some() {
        notices.push(
            "project configuration sets the store view; that is the machine's decision, so it \
             is ignored"
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
        return Err(ConfigError::ParseError(
            "project configuration must not set setup consent; grant it in \
             ~/.config/agentkit/litterbox/config.toml"
                .to_string(),
        ));
    }

    // A local definitions list replaces the project's definitions but leaves
    // each backend sequence in place unless the local file supplied that
    // backend's list. An explicitly empty list removes the project's steps, so
    // its sequences go with them; a non-empty list that drops an identity still
    // fails validation by naming the dangling identity.
    let remove_inherited_sequences = local.provision.as_ref().is_some_and(Vec::is_empty);
    let mut backends = local.backends;
    let mut backend_sequences = BTreeMap::new();

    for (name, value) in &mut backends {
        if let toml::Value::Table(table) = value
            && let Some(sequence) = take_provision(name, table)?
        {
            backend_sequences.insert(name.clone(), sequence);
        }
    }

    for (name, value) in &project.backends {
        let Some(table) = value.as_table() else {
            continue;
        };
        let mut table = table.clone();
        let sequence = take_provision(name, &mut table)?;
        if !table.is_empty() {
            let name = escaped(name);
            notices.push(format!(
                "project configuration configures a backend ('{name}'); backend settings \
                 belong to the machine, so they are ignored"
            ));
        }
        if let Some(sequence) = sequence
            && !remove_inherited_sequences
        {
            backend_sequences.entry(name.clone()).or_insert(sequence);
        }
    }

    Ok(Config {
        backend: local.backend,
        backend_settings: backends,
        project: ProjectConfig {
            slug: local.project.slug.or(project.project.slug),
            backends: local.project.backends.or(project.project.backends),
        },
        provision_definitions: local.provision.or(project.provision).unwrap_or_default(),
        backend_sequences,
        ports: if local.ports.is_empty() {
            project.ports
        } else {
            local.ports
        },
        forwarding: local.forwarding.unwrap_or_default(),
        store_view: local.store_view.unwrap_or_default(),
        accept_degraded: local.accept_degraded,
        allow_project_setup: local.allow_project_setup,
        allowed_projects: local.allowed_projects,
        git: GitConfig {
            snapshot_mode: local.git.snapshot_mode.or(project.git.snapshot_mode),
        },
        notices,
    })
}

/// Take a backend's `provision` sequence out of its table.
fn take_provision(
    backend: &str,
    table: &mut toml::value::Table,
) -> Result<Option<Vec<String>>, ConfigError> {
    let Some(value) = table.remove("provision") else {
        return Ok(None);
    };
    let backend = escaped(backend);
    let toml::Value::Array(values) = value else {
        return Err(ConfigError::ParseError(format!(
            "backends.{backend}.provision must be an array of step identities"
        )));
    };
    let mut sequence = Vec::new();
    for value in values {
        match value {
            toml::Value::String(identity) => sequence.push(identity),
            _ => {
                return Err(ConfigError::ParseError(format!(
                    "backends.{backend}.provision must contain only step identities"
                )));
            }
        }
    }
    Ok(Some(sequence))
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("File not found: {0}")]
    FileNotFound(std::path::PathBuf),
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Provisioning configuration: {0}")]
    Provisioning(ProvisioningIssue),
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

    use super::{
        Config, ConfigError, ForwardingConfig, PortRequest, ProjectConfig, ProjectFile,
        ProvisioningIssue, StoreView, merge, take_provision,
    };

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
                backends: None,
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
        let config = merge(local(), project()).expect("merge");
        assert_eq!(config.backend.as_deref(), Some("docker"));
        assert!(config.backend_settings.contains_key("docker"));
        assert_eq!(config.project.slug.as_deref(), Some("project"));
        assert_eq!(config.ports.len(), 1);
        assert_eq!(config.forwarding.bind, "127.0.0.1");
        assert!(config.notices.is_empty());
    }

    #[test]
    fn a_project_naming_a_backend_is_reported_and_ignored() {
        let mut project = project();
        project.backend = Some("bubblewrap".to_string());
        let config = merge(local(), project).expect("merge");
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
        let mut table = toml::value::Table::new();
        table.insert(
            "image".to_string(),
            toml::Value::String("busybox".to_string()),
        );
        project
            .backends
            .insert("docker".to_string(), toml::Value::Table(table));
        let config = merge(local(), project).expect("merge");
        assert_eq!(config.backend_settings.len(), 1);
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
        let config = merge(local(), project).expect("merge");
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
        let config = merge(local, project()).expect("merge");

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
                slug: Some("local".to_string()),
                backends: None,
            },
            ..ProjectFile::default()
        };
        let config = merge(local, project()).expect("merge");
        assert_eq!(config.project.slug.as_deref(), Some("local"));
    }

    #[test]
    fn forwarding_defaults_to_loopback() {
        let config = merge(ProjectFile::default(), project()).expect("merge");
        assert_eq!(config.forwarding.bind, "127.0.0.1");
    }

    #[test]
    fn a_missing_file_is_distinguishable() {
        let error = ConfigError::FileNotFound("/nowhere".into());
        assert!(error.to_string().contains("/nowhere"));
    }

    #[test]
    fn provision_definitions_and_sequences_parse() {
        let text = r#"
[project]
backends = ["bubblewrap", "docker"]

[[provision]]
identity = "fetch"
kind = "command"
run = "cargo fetch"

[backends.bubblewrap]
provision = ["fetch"]
"#;
        let file: ProjectFile = toml::from_str(text).expect("provisioning config parses");
        assert_eq!(
            file.project.backends.as_deref(),
            Some(["bubblewrap".to_string(), "docker".to_string()].as_slice())
        );
        let definitions = file.provision.as_ref().expect("definitions");
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].identity(), "fetch");
        assert_eq!(definitions[0].kind(), "command");

        let config = merge(ProjectFile::default(), file).expect("merge");
        assert_eq!(config.backend_sequences["bubblewrap"], vec!["fetch"]);
        assert!(
            config.backend_settings.is_empty(),
            "a project table contributes no machine settings"
        );
        assert_eq!(
            config.project.backends.as_deref(),
            Some(["bubblewrap".to_string(), "docker".to_string()].as_slice())
        );
    }

    #[test]
    fn the_store_view_is_machine_owned() {
        let project: ProjectFile = toml::from_str("store-view = 'whole'\n").expect("parses");
        let local: ProjectFile = toml::from_str("store-view = 'closure'\n").expect("parses");
        let config = merge(local, project).expect("merge");
        assert_eq!(config.store_view, StoreView::Closure);
        assert!(
            config
                .notices
                .iter()
                .any(|notice| notice.contains("store view")),
            "{:?}",
            config.notices
        );

        let config = merge(
            ProjectFile {
                store_view: Some(StoreView::Whole),
                ..Default::default()
            },
            ProjectFile::default(),
        )
        .expect("merge");
        assert_eq!(config.store_view, StoreView::Whole);
    }

    #[test]
    fn the_local_override_replaces_a_backend_sequence() {
        let project: ProjectFile = toml::from_str(
            r#"
[backends.bubblewrap]
provision = ["project-step"]
"#,
        )
        .expect("project");
        let local: ProjectFile = toml::from_str(
            r#"
[backends.bubblewrap]
provision = ["local-step"]
"#,
        )
        .expect("local");
        let config = merge(local, project).expect("merge");
        assert_eq!(config.backend_sequences["bubblewrap"], vec!["local-step"]);
    }

    #[test]
    fn a_project_backend_table_contributes_only_provision() {
        let project: ProjectFile = toml::from_str(
            r#"
[backends.bubblewrap]
provision = ["fetch"]
image = "nope"
"#,
        )
        .expect("project");
        let config = merge(ProjectFile::default(), project).expect("merge");
        assert_eq!(config.backend_sequences["bubblewrap"], vec!["fetch"]);
        assert!(
            config.backend_settings.is_empty(),
            "project machine settings are not carried"
        );
        assert!(
            config
                .notices
                .iter()
                .any(|notice| notice.contains("bubblewrap") && notice.contains("ignored")),
            "{:?}",
            config.notices
        );
    }

    #[test]
    fn an_unknown_provisioning_kind_parses_and_is_kept_for_validation() {
        let text = r#"
[[provision]]
identity = "x"
kind = "nix"
installable = "./nix"
"#;
        let file: ProjectFile = toml::from_str(text).expect("the generic step accepts the kind");
        let definitions = file.provision.expect("definitions");
        assert_eq!(definitions[0].kind(), "nix");
        assert_eq!(
            definitions[0]
                .settings
                .get("installable")
                .and_then(toml::Value::as_str),
            Some("./nix")
        );
    }

    #[test]
    fn a_backend_sequence_that_is_not_an_array_is_refused() {
        let project: ProjectFile =
            toml::from_str("[backends.bubblewrap]\nprovision = 'fetch'\n").expect("parses");
        let error = merge(ProjectFile::default(), project).expect_err("non-array refused");
        let message = error.to_string();
        assert!(message.contains("bubblewrap"), "{message}");
        assert!(message.contains("provision"), "{message}");
    }

    #[test]
    fn a_sequence_entry_that_is_not_an_identity_is_refused() {
        let project: ProjectFile =
            toml::from_str("[backends.bubblewrap]\nprovision = ['fetch', 7]\n").expect("parses");
        let error = merge(ProjectFile::default(), project).expect_err("non-string refused");
        let message = error.to_string();
        assert!(message.contains("bubblewrap"), "{message}");
        assert!(message.contains("provision"), "{message}");
    }

    #[test]
    fn take_provision_returns_none_when_the_key_is_absent() {
        let mut table = toml::value::Table::new();
        let sequence = take_provision("bubblewrap", &mut table).expect("no key");
        assert!(sequence.is_none());
        assert!(table.is_empty());
    }

    #[test]
    fn the_local_supported_set_replaces_the_project_one() {
        let project: ProjectFile =
            toml::from_str("[project]\nbackends = ['bubblewrap']\n").expect("project");
        let local: ProjectFile =
            toml::from_str("[project]\nbackends = ['docker']\n").expect("local");
        let config = merge(local, project).expect("merge");
        assert_eq!(
            config.project.backends.as_deref(),
            Some(["docker".to_string()].as_slice())
        );
    }

    #[test]
    fn the_local_definitions_replace_the_project_definitions() {
        let project: ProjectFile = toml::from_str(
            "[[provision]]\nidentity = 'project-step'\nkind = 'command'\nrun = 'true'\n",
        )
        .expect("project");
        let local: ProjectFile = toml::from_str(
            "[[provision]]\nidentity = 'local-step'\nkind = 'command'\nrun = 'true'\n",
        )
        .expect("local");
        let config = merge(local, project).expect("merge");
        assert_eq!(config.provision_definitions.len(), 1);
        assert_eq!(config.provision_definitions[0].identity(), "local-step");
    }

    #[test]
    fn an_empty_local_definition_list_removes_inherited_sequences() {
        let project: ProjectFile = toml::from_str(
            "[[provision]]\nidentity = 'fetch'\nkind = 'command'\nrun = 'true'\n\
             [backends.bubblewrap]\nprovision = ['fetch']\n",
        )
        .expect("project");
        let local: ProjectFile = toml::from_str("provision = []\n").expect("local");
        let mut config = merge(local, project).expect("merge");
        assert!(config.provision_definitions.is_empty());
        assert!(
            config.backend_sequences.is_empty(),
            "the empty list removes the inherited sequence: {:?}",
            config.backend_sequences
        );
        config
            .validate_provisioning()
            .expect("an empty local definition list is loadable");
    }

    #[test]
    fn an_empty_local_sequence_removes_the_project_sequence() {
        let project: ProjectFile =
            toml::from_str("[backends.bubblewrap]\nprovision = ['fetch']\n").expect("project");
        let local: ProjectFile =
            toml::from_str("[backends.bubblewrap]\nprovision = []\n").expect("local");
        let config = merge(local, project).expect("merge");
        assert_eq!(config.backend_sequences["bubblewrap"], Vec::<String>::new());
    }

    #[test]
    fn local_machine_flags_reach_the_merged_configuration() {
        let local = ProjectFile {
            accept_degraded: true,
            allow_project_setup: true,
            allowed_projects: vec!["project".to_string()],
            ..ProjectFile::default()
        };
        let config = merge(local, ProjectFile::default()).expect("merge");
        assert!(config.accept_degraded);
        assert!(config.allow_project_setup);
        assert_eq!(config.allowed_projects, vec!["project"]);
    }

    #[test]
    fn a_project_setting_allowed_projects_is_refused() {
        let project: ProjectFile =
            toml::from_str("allowed-projects = ['project']\n").expect("project");
        let error = merge(ProjectFile::default(), project).expect_err("consent refused");
        assert!(
            error.to_string().contains("must not set setup consent"),
            "{error}"
        );
        assert!(
            error
                .to_string()
                .contains("~/.config/agentkit/litterbox/config.toml"),
            "{error}"
        );
    }

    #[test]
    fn a_project_accepting_degraded_mechanisms_is_noticed_and_ignored() {
        let project: ProjectFile = toml::from_str("accept-degraded = true\n").expect("project");
        let config = merge(ProjectFile::default(), project).expect("merge");
        assert!(!config.accept_degraded);
        assert!(
            config
                .notices
                .iter()
                .any(|notice| notice.contains("accepts degraded")),
            "{:?}",
            config.notices
        );
    }

    #[test]
    fn a_non_empty_local_definition_list_keeps_project_sequences() {
        let project: ProjectFile = toml::from_str(
            "[[provision]]\nidentity = 'fetch'\nkind = 'command'\nrun = 'true'\n\
             [backends.bubblewrap]\nprovision = ['fetch']\n",
        )
        .expect("project");
        let local: ProjectFile =
            toml::from_str("[[provision]]\nidentity = 'fetch'\nkind = 'command'\nrun = 'false'\n")
                .expect("local");
        let config = merge(local, project).expect("merge");
        assert_eq!(config.backend_sequences["bubblewrap"], vec!["fetch"]);
    }

    #[test]
    fn snapshot_mode_prefers_the_local_file_then_the_project() {
        let project: ProjectFile =
            toml::from_str("[git]\nsnapshot-mode = 'direct'\n").expect("project");
        let config = merge(ProjectFile::default(), project.clone()).expect("merge");
        assert_eq!(
            config.git.snapshot_mode,
            Some(crate::domain::ScmMode::Direct)
        );

        let local: ProjectFile =
            toml::from_str("[git]\nsnapshot-mode = 'remote'\n").expect("local");
        let config = merge(local, project).expect("merge");
        assert_eq!(
            config.git.snapshot_mode,
            Some(crate::domain::ScmMode::Remote)
        );
    }

    #[test]
    fn backend_name_boundaries_are_enforced() {
        assert!(super::valid_backend_name(&"a".repeat(64)));
        assert!(!super::valid_backend_name(&"a".repeat(65)));
        assert!(!super::valid_backend_name("\nbad"));
        assert!(super::valid_backend_name("a.b-c_d+e"));
    }

    #[test]
    fn a_local_backend_table_keeps_machine_settings_and_extracts_provision() {
        let local: ProjectFile =
            toml::from_str("[backends.docker]\nimage = 'busybox'\nprovision = ['fetch']\n")
                .expect("local");
        let config = merge(local, project()).expect("merge");
        assert_eq!(config.backend_sequences["docker"], vec!["fetch"]);
        let docker = config.backend_settings["docker"]
            .as_table()
            .expect("docker table");
        assert_eq!(
            docker.get("image").and_then(toml::Value::as_str),
            Some("busybox"),
            "the machine setting stays for the backend"
        );
        assert!(
            !docker.contains_key("provision"),
            "provision is not left for deny_unknown_fields"
        );
    }

    #[test]
    fn a_provisioning_issue_display_carries_the_remedy() {
        let issue = ProvisioningIssue {
            backend: None,
            step: None,
            reason: "the project declares too many steps".to_string(),
            remedy: "remove steps the project does not need".to_string(),
        };
        assert_eq!(
            issue.to_string(),
            "the project declares too many steps; remove steps the project does not need"
        );
    }

    #[test]
    fn an_invalid_sequence_key_is_refused_and_escaped() {
        let config = Config {
            backend_sequences: BTreeMap::from([(
                "bad\nname".to_string(),
                vec!["fetch".to_string()],
            )]),
            ..Config::default()
        };
        let error = config
            .check_provisioning()
            .expect_err("the invalid key is refused");
        assert!(error.reason.contains("bad\\nname"), "{error}");
        assert!(!error.reason.contains('\n'), "{error}");
    }
}
