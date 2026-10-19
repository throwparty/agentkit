//! The definition types: a parsed persona, actor, or prompt with its frontmatter, and the collection discovered at startup.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum DefinitionError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: definition exceeds the {max}-byte size cap")]
    TooLarge { path: PathBuf, max: usize },
    #[error("{path}: missing frontmatter fence (--- or +++)")]
    MissingFrontmatter { path: PathBuf },
    #[error("{path}: unterminated frontmatter fence")]
    UnterminatedFrontmatter { path: PathBuf },
    #[error("{path}: invalid frontmatter: {message}")]
    Frontmatter { path: PathBuf, message: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Definition<T> {
    /// Definition name: the file stem, addressable in its namespace.
    pub name: String,
    /// Where the definition was loaded from.
    pub path: PathBuf,
    /// The raw file contents — the unit hashed by TOFU trust records.
    pub raw: String,
    /// Parsed frontmatter.
    pub frontmatter: T,
    /// The markdown body: the persona's behavioural prompt, or the
    /// prompt's expansion template.
    pub body: String,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Definitions {
    pub personas: BTreeMap<String, Definition<PersonaMeta>>,
    pub actors: BTreeMap<String, Definition<ActorMeta>>,
    pub prompts: BTreeMap<String, Definition<PromptMeta>>,
    /// Built-in definitions a discovered definition replaced (as
    /// `kind:name`) — surfaced at first run.
    pub builtin_overrides: BTreeSet<String>,
}

impl Definitions {
    /// Trust entries for every project-layer definition: relative path and
    /// SHA-256 of the raw content.
    pub fn project_trust_entries(&self, project_dir: &Path) -> BTreeMap<String, String> {
        let mut entries = BTreeMap::new();
        Self::collect_trust_entries(&self.personas, project_dir, &mut entries);
        Self::collect_trust_entries(&self.actors, project_dir, &mut entries);
        Self::collect_trust_entries(&self.prompts, project_dir, &mut entries);
        entries
    }

    fn collect_trust_entries<T>(
        map: &BTreeMap<String, Definition<T>>,
        project_dir: &Path,
        entries: &mut BTreeMap<String, String>,
    ) {
        for def in map.values() {
            if let Ok(rel) = def.path.strip_prefix(project_dir) {
                entries.insert(
                    rel.to_string_lossy().to_string(),
                    crate::config::trust::hash(&def.raw),
                );
            }
        }
    }

    /// Drops project-layer definitions whose trust entries were refused.
    pub fn retain_project(&mut self, project_dir: &Path, approved: &BTreeSet<String>) {
        Self::retain_map(&mut self.personas, project_dir, approved);
        Self::retain_map(&mut self.actors, project_dir, approved);
        Self::retain_map(&mut self.prompts, project_dir, approved);
    }

    fn retain_map<T>(
        map: &mut BTreeMap<String, Definition<T>>,
        project_dir: &Path,
        approved: &BTreeSet<String>,
    ) {
        map.retain(|_, def| {
            match def.path.strip_prefix(project_dir) {
                Ok(rel) => approved.contains(&rel.to_string_lossy().to_string()),
                Err(_) => true, // not a project-layer definition
            }
        });
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PersonaMeta {
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ActorMeta {
    /// The persona this actor runs as.
    pub persona: String,
    /// Endpoint-qualified model override; falls back to `[defaults].model`.
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
pub struct PromptMeta {
    #[serde(default)]
    pub description: Option<String>,
    /// Declared parameters, substituted positionally into `{{ name }}`
    /// placeholders: each parameter takes the next whitespace token,
    /// except the last, which takes the remainder of the arguments.
    #[serde(default)]
    pub parameters: Vec<String>,
    /// Invocable by the user as a slash command. Defaults to true (the
    /// Pi convention: the filename is the command).
    #[serde(default = "default_true")]
    pub user_invokable: bool,
    /// Exposed to the model as a tool whose invocation loads the body.
    #[serde(default)]
    pub model_invokable: bool,
    /// Routes invocation to the compaction script instead of a model turn.
    #[serde(default)]
    pub compaction: bool,
}

fn default_true() -> bool {
    true
}
