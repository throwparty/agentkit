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
    #[error("{path}: directory definition is missing its {entry} entry point")]
    MissingEntryPoint { path: PathBuf, entry: &'static str },
    #[error("{name}: flat definition {flat} collides with directory definition {directory}")]
    FlatDirectoryCollision {
        name: String,
        flat: PathBuf,
        directory: PathBuf,
    },
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
    /// Where the definition was loaded from: the entry point for a
    /// package, or the single file for a flat definition.
    pub path: PathBuf,
    /// The package directory for a directory definition, repository
    /// relative. `None` for a flat definition, which carries no
    /// resources.
    pub directory: Option<PathBuf>,
    /// Resource paths relative to [`Self::directory`], sorted so the
    /// listing and the lock hash are deterministic. Always empty for a
    /// flat definition and for built-ins.
    pub resources: Vec<String>,
    /// The raw entry-point contents — the unit hashed by TOFU trust
    /// records.
    pub raw: String,
    /// Parsed frontmatter.
    pub frontmatter: T,
    /// The markdown body: the persona's behavioural prompt, or the
    /// prompt's expansion template.
    pub body: String,
}

/// Enumerates the resource files in a package directory, relative to it
/// and sorted. The entry point is excluded: it is the definition, not a
/// resource. Directory entries are walked recursively; symbolic links are
/// not followed.
pub(crate) fn collect_resources(
    directory: &Path,
    entry: &Path,
) -> Result<Vec<String>, DefinitionError> {
    let mut resources = Vec::new();
    walk_resources(directory, directory, entry, &mut resources)?;
    resources.sort();
    Ok(resources)
}

fn walk_resources(
    root: &Path,
    dir: &Path,
    entry: &Path,
    out: &mut Vec<String>,
) -> Result<(), DefinitionError> {
    let entries = std::fs::read_dir(dir).map_err(|source| DefinitionError::Io {
        path: dir.into(),
        source,
    })?;
    for item in entries {
        let item = item.map_err(|source| DefinitionError::Io {
            path: dir.into(),
            source,
        })?;
        let path = item.path();
        let file_type = item.file_type().map_err(|source| DefinitionError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            walk_resources(root, &path, entry, out)?;
        } else if file_type.is_file() && path != entry {
            let rel = path.strip_prefix(root).expect("walk stays under root");
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// The total size of every file in a package, entry point included. A flat
/// definition is a package of one.
pub(crate) fn package_size(directory: &Path) -> Result<usize, DefinitionError> {
    let mut total = 0usize;
    let entries = std::fs::read_dir(directory).map_err(|source| DefinitionError::Io {
        path: directory.into(),
        source,
    })?;
    for item in entries {
        let item = item.map_err(|source| DefinitionError::Io {
            path: directory.into(),
            source,
        })?;
        let path = item.path();
        let file_type = item.file_type().map_err(|source| DefinitionError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            total += package_size(&path)?;
        } else if file_type.is_file() {
            total += item
                .metadata()
                .map_err(|source| DefinitionError::Io {
                    path: path.clone(),
                    source,
                })?
                .len() as usize;
        }
    }
    Ok(total)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_definition_reports_sorted_resources() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("package");
        std::fs::create_dir_all(package.join("nested")).unwrap();
        std::fs::write(package.join("PERSONA.md"), "body").unwrap();
        std::fs::write(package.join("z.txt"), "z").unwrap();
        std::fs::write(package.join("a.txt"), "a").unwrap();
        std::fs::write(package.join("nested/b.txt"), "b").unwrap();

        let entry = package.join("PERSONA.md");
        let resources = collect_resources(&package, &entry).unwrap();
        assert_eq!(resources, vec!["a.txt", "nested/b.txt", "z.txt"]);

        let definition = Definition {
            name: "package".to_owned(),
            path: entry.clone(),
            directory: Some(PathBuf::from(".agentkit/tackle/personas/package")),
            resources,
            raw: "body".to_owned(),
            frontmatter: PersonaMeta { description: None },
            body: "body".to_owned(),
        };
        assert_eq!(
            definition.directory.as_deref(),
            Some(Path::new(".agentkit/tackle/personas/package"))
        );
        assert_eq!(definition.resources.len(), 3);
    }

    #[test]
    fn flat_definition_reports_no_directory_or_resources() {
        let definition = Definition {
            name: "flat".to_owned(),
            path: PathBuf::from("flat.md"),
            directory: None,
            resources: Vec::new(),
            raw: "body".to_owned(),
            frontmatter: PersonaMeta { description: None },
            body: "body".to_owned(),
        };
        assert!(definition.directory.is_none());
        assert!(definition.resources.is_empty());
    }

    #[test]
    fn resource_enumeration_is_order_stable() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("package");
        std::fs::create_dir_all(&package).unwrap();
        for name in ["c", "a", "b"] {
            std::fs::write(package.join(format!("{name}.md")), name).unwrap();
        }
        let entry = package.join("ENTRY.md");
        std::fs::write(&entry, "entry").unwrap();
        let first = collect_resources(&package, &entry).unwrap();
        let second = collect_resources(&package, &entry).unwrap();
        assert_eq!(first, second);
        assert_eq!(first, vec!["a.md", "b.md", "c.md"]);
    }

    #[test]
    fn package_size_counts_every_file() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("package");
        std::fs::create_dir_all(package.join("nested")).unwrap();
        std::fs::write(package.join("ENTRY.md"), "1234").unwrap();
        std::fs::write(package.join("nested/resource"), "12").unwrap();
        let size = package_size(&package).unwrap();
        assert_eq!(size, 6);
    }
}
