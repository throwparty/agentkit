//! The configuration schema: the user/project layers and the typed fields they carry.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

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
        /// Extra request headers, sent with every MCP request.
        #[serde(default)]
        headers: BTreeMap<String, String>,
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
