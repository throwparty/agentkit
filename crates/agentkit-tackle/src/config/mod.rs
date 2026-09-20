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
use std::collections::BTreeMap;
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
}

impl Config {
    /// Merges a project layer over this (user) layer with field-level
    /// precedence: project values win where present.
    fn merge(mut self, project: Config) -> Config {
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
                gate_project_entries(gate, project)
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

/// Trust-gates the executable entries of a project layer (`mcp_servers`
/// and `scripts`); unapproved entries are dropped before the merge. The
/// hash covers the serialised entry, so any field change re-prompts.
fn gate_project_entries(gate: &mut trust::Gate<'_>, project: Config) -> Config {
    let mut entries = BTreeMap::new();
    for (name, server) in &project.mcp_servers {
        let serialised = toml::to_string(server).unwrap_or_default();
        entries.insert(format!("mcp_servers/{name}"), trust::hash(&serialised));
    }
    for (name, script) in &project.scripts {
        let serialised = toml::to_string(script).unwrap_or_default();
        entries.insert(format!("scripts/{name}"), trust::hash(&serialised));
    }
    if entries.is_empty() {
        return project;
    }
    let approved = gate.gate(&entries);
    let mut project = project;
    project
        .mcp_servers
        .retain(|name, _| approved.contains(&format!("mcp_servers/{name}")));
    project
        .scripts
        .retain(|name, _| approved.contains(&format!("scripts/{name}")));
    project
}

fn load_file(path: &Path) -> Result<Option<Config>, ConfigError> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ConfigError::Io {
                path: path.into(),
                source,
            })
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
    Ok(())
}

// --- Schema (config.example.toml is the human-readable schema of record) ---

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WireFormat {
    OpenaiChatCompletions,
    OpenaiResponses,
    AnthropicMessages,
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
}
