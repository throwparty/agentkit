//! Layered configuration: a user layer and a project layer, merged with
//! project precedence.
//!
//! Security-relevant fields — model endpoints and the credential helper —
//! are user-configuration only: a project layer declaring them is rejected
//! with a named error, so a cloned repository cannot redirect model traffic
//! or credential resolution (Scout finding 2). Project `mcp_servers`
//! entries are accepted but trust-gated (see [`trust`]).

pub mod trust;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Config {
    pub credential_helper: Option<String>,
    pub endpoints: BTreeMap<String, EndpointConfig>,
    pub defaults: Defaults,
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
    pub scripts: BTreeMap<String, ScriptConfig>,
    pub turns: TurnsConfig,
    pub sessions: SessionsConfig,
    /// The ACP client's file-system and terminal tools. User
    /// configuration only: a cloned repository must not be able to hand
    /// the model its editor's shell or filesystem.
    pub client_tools: Option<ClientToolsConfig>,
}

impl Config {
    /// Merges a project layer over this (user) layer with field-level
    /// precedence: project values win where present.
    pub(crate) fn merge(mut self, project: Config) -> Config {
        if project.credential_helper.is_some() {
            self.credential_helper = project.credential_helper;
        }
        self.endpoints.extend(project.endpoints);
        self.defaults.actor = project.defaults.actor.or(self.defaults.actor);
        self.defaults.model = project.defaults.model.or(self.defaults.model);
        self.mcp_servers.extend(project.mcp_servers);
        self.scripts.extend(project.scripts);
        self.turns.max_model_requests = project
            .turns
            .max_model_requests
            .or(self.turns.max_model_requests);
        self.sessions.include_ephemeral = project
            .sessions
            .include_ephemeral
            .or(self.sessions.include_ephemeral);
        // Project layers are rejected outright (above), so this only ever
        // carries the user layer's section through the builtin seed.
        self.client_tools = project.client_tools.or(self.client_tools);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Loaded {
    pub config: Config,
    /// The user layer file, if one existed.
    pub user_layer: Option<PathBuf>,
    /// The project layer file, if one existed.
    pub project_layer: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{path}: project configuration must not set `{key}` (user configuration only)")]
    ProjectForbidden { path: PathBuf, key: &'static str },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Loads the layered configuration: the user layer from `user_dir/config.toml`
/// (if present), overlaid by the project layer from
/// `project_dir/config.toml` (if present, and subject to endpoint rejection
/// plus trust gating of executable entries — `mcp_servers` and `scripts` —
/// before the merge, so shadowed user entries resurface when a project
/// entry is refused).
pub fn load_layered(
    user_dir: &Path,
    project_dir: Option<&Path>,
    gate: Option<&mut trust::Gate<'_>>,
) -> Result<Loaded, ConfigError> {
    let user_path = user_dir.join("config.toml");
    let user = load_file(&user_path)?;

    let project_path = project_dir.map(|d| d.join("config.toml"));
    let project_layer = project_path.filter(|p| p.is_file());
    let project = match &project_layer {
        Some(path) => {
            let project = load_file(path)?.unwrap_or_default();
            reject_project_security_fields(&project, path)?;
            if let Some(gate) = gate {
                let project_dir = path.parent().unwrap_or_else(|| Path::new("."));
                gate_project_entries(gate, project, project_dir)
            } else {
                project
            }
        }
        None => Config::default(),
    };

    Ok(Loaded {
        // The built-in registrations seed at the lowest precedence: a
        // user or project entry with the same name replaces one
        // wholesale (a disabling entry needs no file).
        config: builtin_registrations()
            .merge(user.unwrap_or_default())
            .merge(project),
        user_layer: user_path.is_file().then_some(user_path),
        project_layer,
    })
}

/// Loads the built-in registrations and the user layer, returning the raw
/// project layer separately. The project layer is *not* merged: its
/// executable entries are trust-gated, and consent is resolved at the
/// first session (there is no ACP connection at startup), so the caller
/// holds it until then.
pub fn load_layered_deferred(
    user_dir: &Path,
    project_dir: Option<&Path>,
) -> Result<(Loaded, Option<Config>), ConfigError> {
    let user_path = user_dir.join("config.toml");
    let user = load_file(&user_path)?;

    let project_path = project_dir.map(|d| d.join("config.toml"));
    let project_layer = project_path.filter(|p| p.is_file());
    let project = match &project_layer {
        Some(path) => {
            let project = load_file(path)?.unwrap_or_default();
            // Endpoint/credential rejection is not trust-gated: it holds
            // regardless of consent, so it fails at load.
            reject_project_security_fields(&project, path)?;
            Some(project)
        }
        None => None,
    };

    Ok((
        Loaded {
            config: builtin_registrations().merge(user.unwrap_or_default()),
            user_layer: user_path.is_file().then_some(user_path),
            project_layer,
        },
        project,
    ))
}

/// The shipped script registrations: compaction (automatic past the
/// utilisation threshold and the manual /compact summary) and titling.
/// Sources are built-in (`builtin:` references); user configuration
/// overrides any entry wholesale.
fn builtin_registrations() -> Config {
    let mut config = Config::default();
    config.scripts.insert(
        "compaction".to_owned(),
        ScriptConfig {
            events: vec!["post_turn".to_owned(), "compaction_requested".to_owned()],
            file: "builtin:compaction.rhai".to_owned(),
            enabled: true,
        },
    );
    config.scripts.insert(
        "titling".to_owned(),
        ScriptConfig {
            events: vec!["title_trigger".to_owned()],
            file: "builtin:titling.rhai".to_owned(),
            enabled: true,
        },
    );
    config
}

/// The trust entries for a project config's executable sections
/// (`mcp_servers` and `scripts`): relative path and SHA-256 of the
/// serialised entry.
pub fn project_trust_entries(project: &Config) -> BTreeMap<String, String> {
    let mut entries = BTreeMap::new();
    for (name, server) in &project.mcp_servers {
        let serialised = toml::to_string(server).unwrap_or_default();
        entries.insert(format!("mcp_servers/{name}"), trust::hash(&serialised));
    }
    for (name, script) in &project.scripts {
        let serialised = toml::to_string(script).unwrap_or_default();
        entries.insert(format!("scripts/{name}"), trust::hash(&serialised));
    }
    entries
}

/// Drops the executable project config entries whose trust entries were
/// refused, and pins each retained project script to its project-layer
/// file. The pin records the script's origin explicitly: resolution reads
/// the approved file directly, so a user-layer file of the same name
/// cannot shadow it, and a trusted project script is no longer dropped.
pub fn retain_project_entries(
    mut project: Config,
    project_dir: &Path,
    approved: &BTreeSet<String>,
) -> Config {
    project
        .mcp_servers
        .retain(|name, _| approved.contains(&format!("mcp_servers/{name}")));
    project
        .scripts
        .retain(|name, _| approved.contains(&format!("scripts/{name}")));
    for script in project.scripts.values_mut() {
        // A `builtin:` reference is not a project file: leave it to the
        // built-in resolver.
        if !script.file.starts_with("builtin:") {
            script.file = project_dir
                .join("scripts")
                .join(&script.file)
                .to_string_lossy()
                .into_owned();
        }
    }
    project
}

/// Resolves a registered script's source.
///
/// The reference decides the layer: a `builtin:<name>.rhai` reference is
/// the embedded source, an absolute path is a project-layer script pinned
/// at trust resolution, and anything else is a filename under the user
/// layer's `scripts/` directory. An unreadable file is logged and skipped.
pub fn resolve_script_source(name: &str, file: &str, user_dir: Option<&Path>) -> Option<String> {
    if let Some(source) = crate::builtins::script_source(file) {
        return Some(source.to_owned());
    }
    let resolved = if Path::new(file).is_absolute() {
        PathBuf::from(file)
    } else {
        user_dir?.join("scripts").join(file)
    };
    match std::fs::read_to_string(&resolved) {
        Ok(source) => Some(source),
        Err(err) => {
            tracing::warn!(script = %name, path = %resolved.display(), "script unreadable: {err}");
            None
        }
    }
}

/// Trust-gates the executable entries of a project layer (`mcp_servers`
/// and `scripts`); unapproved entries are dropped before the merge. The
/// hash covers the serialised entry, so any field change re-prompts.
fn gate_project_entries(gate: &mut trust::Gate<'_>, project: Config, project_dir: &Path) -> Config {
    let entries = project_trust_entries(&project);
    if entries.is_empty() {
        return project;
    }
    let approved = gate.gate(&entries);
    retain_project_entries(project, project_dir, &approved)
}

fn load_file(path: &Path) -> Result<Option<Config>, ConfigError> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ConfigError::Io {
                path: path.into(),
                source,
            });
        }
    };
    toml::from_str(&content)
        .map(Some)
        .map_err(|source| ConfigError::Parse {
            path: path.into(),
            source,
        })
}

fn reject_project_security_fields(project: &Config, path: &Path) -> Result<(), ConfigError> {
    if !project.endpoints.is_empty() {
        return Err(ConfigError::ProjectForbidden {
            path: path.to_path_buf(),
            key: "endpoints",
        });
    }
    if project.credential_helper.is_some() {
        return Err(ConfigError::ProjectForbidden {
            path: path.to_path_buf(),
            key: "credential_helper",
        });
    }
    if project.client_tools.is_some() {
        return Err(ConfigError::ProjectForbidden {
            path: path.to_path_buf(),
            key: "client_tools",
        });
    }
    Ok(())
}

// --- Schema (the types below are the source of truth for the TOML shape) ---

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EndpointConfig {
    pub base_url: String,
    pub wire_format: WireFormat,
    pub auth: Auth,
    /// Static model list, used as the fallback when `/models` discovery
    /// fails or returns nothing.
    #[serde(default)]
    pub models: Vec<String>,
}

/// The endpoint's wire format. v1 implements the OpenAI
/// chat-completions shape only; other formats are not advertised until
/// they are implemented, so a configuration cannot select one that
/// cannot serve a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WireFormat {
    OpenaiChatCompletions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Auth {
    None,
    Helper,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "lowercase")]
pub enum McpServerConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptConfig {
    pub events: Vec<String>,
    pub file: String,
    /// Entries may disable a script — built-ins included — without a file.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
pub struct Defaults {
    pub actor: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
pub struct TurnsConfig {
    pub max_model_requests: Option<u32>,
}

#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
pub struct SessionsConfig {
    pub include_ephemeral: Option<bool>,
}

/// The `[client_tools]` section: which categories of the ACP client's
/// advertised surface are reflected to the model. Both default on when
/// the client advertises the capability; a category the client does not
/// advertise is never exposed regardless.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ClientToolsConfig {
    #[serde(default = "default_true")]
    pub fs_enabled: bool,
    #[serde(default = "default_true")]
    pub terminal_enabled: bool,
}

impl Default for ClientToolsConfig {
    fn default() -> Self {
        Self {
            fs_enabled: true,
            terminal_enabled: true,
        }
    }
}

// The Config tree deserialises from the TOML schema via an intermediate
// raw shape: the same types, but with `Deserialize` applied directly and
// `Config::default()` semantics for absent layers.

impl<'de> Deserialize<'de> for Config {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            credential_helper: Option<String>,
            #[serde(default)]
            endpoints: BTreeMap<String, EndpointConfig>,
            #[serde(default)]
            defaults: Option<Defaults>,
            #[serde(default)]
            mcp_servers: BTreeMap<String, McpServerConfig>,
            #[serde(default)]
            scripts: BTreeMap<String, ScriptConfig>,
            #[serde(default)]
            turns: Option<TurnsConfig>,
            #[serde(default)]
            sessions: Option<SessionsConfig>,
            #[serde(default)]
            client_tools: Option<ClientToolsConfig>,
        }

        let raw = Raw::deserialize(deserializer)?;
        Ok(Config {
            credential_helper: raw.credential_helper,
            endpoints: raw.endpoints,
            defaults: raw.defaults.unwrap_or_default(),
            mcp_servers: raw.mcp_servers,
            scripts: raw.scripts,
            turns: raw.turns.unwrap_or_default(),
            sessions: raw.sessions.unwrap_or_default(),
            client_tools: raw.client_tools,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_layer(dir: &Path, content: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn missing_layers_yield_default_config() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load_layered(
            &dir.path().join("user"),
            Some(&dir.path().join("project")),
            None,
        )
        .unwrap();
        assert!(loaded.config.endpoints.is_empty());
        assert_eq!(loaded.config.defaults.actor, None);
        assert_eq!(loaded.user_layer, None);
        assert_eq!(loaded.project_layer, None);
    }

    #[test]
    fn project_overrides_user_for_defaults() {
        let dir = tempfile::tempdir().unwrap();
        write_layer(
            &dir.path().join("user"),
            "[defaults]\nactor = \"user-actor\"\n",
        );
        write_layer(
            &dir.path().join("project"),
            "[defaults]\nactor = \"project-actor\"\n",
        );
        let loaded = load_layered(
            &dir.path().join("user"),
            Some(&dir.path().join("project")),
            None,
        )
        .unwrap();
        assert_eq!(
            loaded.config.defaults.actor,
            Some("project-actor".to_string())
        );
        assert!(loaded.user_layer.is_some());
        assert!(loaded.project_layer.is_some());
    }

    #[test]
    fn project_mcp_servers_win_on_collision() {
        let dir = tempfile::tempdir().unwrap();
        write_layer(
            &dir.path().join("user"),
            "[mcp_servers.tools]\ntransport = \"http\"\nurl = \"https://user.example.com\"\n",
        );
        write_layer(
            &dir.path().join("project"),
            "[mcp_servers.tools]\ntransport = \"http\"\nurl = \"https://project.example.com\"\n",
        );
        let loaded = load_layered(
            &dir.path().join("user"),
            Some(&dir.path().join("project")),
            None,
        )
        .unwrap();
        match loaded.config.mcp_servers.get("tools") {
            Some(McpServerConfig::Http { url }) => {
                assert_eq!(url, "https://project.example.com")
            }
            other => panic!("unexpected server config: {other:?}"),
        }
    }

    #[test]
    fn unimplemented_wire_formats_are_rejected_at_parse() {
        // v1 implements openai-chat-completions only; a config naming an
        // unimplemented format fails at load rather than at first prompt.
        let dir = tempfile::tempdir().unwrap();
        write_layer(
            &dir.path().join("user"),
            "[endpoints.anthropic]\nbase_url = \"https://api.anthropic.com\"\nwire_format = \"anthropic-messages\"\nauth = \"helper\"\n",
        );
        let err = load_layered(&dir.path().join("user"), None, None).unwrap_err();
        assert!(err.to_string().contains("wire_format"), "{err}");
    }

    #[test]
    fn project_endpoints_are_rejected_by_name() {
        let dir = tempfile::tempdir().unwrap();
        write_layer(
            &dir.path().join("project"),
            "[endpoints.rogue]\nbase_url = \"https://rogue.example.com\"\nwire_format = \"openai-chat-completions\"\nauth = \"none\"\n",
        );
        let err = load_layered(
            &dir.path().join("user"),
            Some(&dir.path().join("project")),
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("endpoints"), "{err}");
    }

    #[test]
    fn project_credential_helper_is_rejected_by_name() {
        let dir = tempfile::tempdir().unwrap();
        write_layer(
            &dir.path().join("project"),
            "credential_helper = \"rogue\"\n",
        );
        let err = load_layered(
            &dir.path().join("user"),
            Some(&dir.path().join("project")),
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("credential_helper"), "{err}");
    }

    #[test]
    fn project_client_tools_are_rejected_by_name() {
        let dir = tempfile::tempdir().unwrap();
        write_layer(
            &dir.path().join("project"),
            "[client_tools]\nfs_enabled = false\n",
        );
        let err = load_layered(
            &dir.path().join("user"),
            Some(&dir.path().join("project")),
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("client_tools"), "{err}");
    }

    #[test]
    fn client_tools_default_on_and_parse_toggles() {
        let dir = tempfile::tempdir().unwrap();
        let absent = load_layered(
            &dir.path().join("user"),
            Some(&dir.path().join("project")),
            None,
        )
        .unwrap();
        assert_eq!(
            absent.config.client_tools.as_ref(),
            None,
            "an absent section leaves both categories at their defaults"
        );

        write_layer(
            &dir.path().join("user"),
            "[client_tools]\nterminal_enabled = false\n",
        );
        let parsed = load_layered(
            &dir.path().join("user"),
            Some(&dir.path().join("project")),
            None,
        )
        .unwrap();
        let section = parsed.config.client_tools.expect("section parses");
        assert!(section.fs_enabled);
        assert!(!section.terminal_enabled);
    }

    #[test]
    fn user_layer_configures_endpoints() {
        let dir = tempfile::tempdir().unwrap();
        write_layer(
            &dir.path().join("user"),
            "[endpoints.switchboard]\nbase_url = \"http://localhost:3812/openai/v1\"\nwire_format = \"openai-chat-completions\"\nauth = \"none\"\n\n[defaults]\nmodel = \"switchboard/claude-sonnet-4-5\"\n",
        );
        let loaded = load_layered(
            &dir.path().join("user"),
            Some(&dir.path().join("project")),
            None,
        )
        .unwrap();
        assert_eq!(loaded.config.endpoints.len(), 1);
        assert_eq!(
            loaded.config.defaults.model,
            Some("switchboard/claude-sonnet-4-5".to_string())
        );
    }

    #[test]
    fn stdio_mcp_server_with_env_parses() {
        let dir = tempfile::tempdir().unwrap();
        write_layer(
            &dir.path().join("user"),
            "[mcp_servers.litterbox]\ntransport = \"stdio\"\ncommand = \"litterbox\"\nargs = [\"mcp\"]\n\n[mcp_servers.litterbox.env]\nLITTERBOX_LOG = \"debug\"\n",
        );
        let loaded = load_layered(
            &dir.path().join("user"),
            Some(&dir.path().join("project")),
            None,
        )
        .unwrap();
        match loaded.config.mcp_servers.get("litterbox") {
            Some(McpServerConfig::Stdio { command, args, env }) => {
                assert_eq!(command, "litterbox");
                assert_eq!(args, &["mcp"]);
                assert_eq!(env.get("LITTERBOX_LOG").map(String::as_str), Some("debug"));
            }
            other => panic!("unexpected server config: {other:?}"),
        }
    }

    #[test]
    fn script_entry_parses_with_enabled_default() {
        let dir = tempfile::tempdir().unwrap();
        write_layer(
            &dir.path().join("user"),
            "[scripts.deny-secrets]\nevents = [\"pre_tool_use\"]\nfile = \"deny-secrets.rhai\"\n",
        );
        let loaded = load_layered(
            &dir.path().join("user"),
            Some(&dir.path().join("project")),
            None,
        )
        .unwrap();
        let script = loaded.config.scripts.get("deny-secrets").unwrap();
        assert!(script.enabled);
        assert_eq!(script.events, &["pre_tool_use"]);
    }

    #[test]
    fn approved_project_script_resolves_to_its_project_file() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(project.join("scripts")).unwrap();
        std::fs::write(
            project.join("scripts/deny.rhai"),
            "fn pre_tool_use(r) { r }",
        )
        .unwrap();
        // A user-layer file of the same name must not shadow the project
        // script: the pin selects the approved project file.
        let user = dir.path().join("user");
        std::fs::create_dir_all(user.join("scripts")).unwrap();
        std::fs::write(user.join("scripts/deny.rhai"), "user shadow").unwrap();

        let project_config: Config =
            toml::from_str("[scripts.deny]\nevents = [\"pre_tool_use\"]\nfile = \"deny.rhai\"\n")
                .unwrap();
        let approved = BTreeSet::from(["scripts/deny".to_owned()]);
        let retained = retain_project_entries(project_config, &project, &approved);
        let script = retained.scripts.get("deny").unwrap();

        let source = resolve_script_source("deny", &script.file, Some(&user)).unwrap();
        assert_eq!(source, "fn pre_tool_use(r) { r }");
    }

    #[test]
    fn refused_project_script_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let project_config: Config =
            toml::from_str("[scripts.deny]\nevents = [\"pre_tool_use\"]\nfile = \"deny.rhai\"\n")
                .unwrap();
        let retained = retain_project_entries(project_config, &project, &BTreeSet::new());
        assert!(retained.scripts.is_empty());
    }
}
