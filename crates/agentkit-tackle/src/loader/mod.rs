//! Definition loading: personas, actors, and prompts.
//!
//! Definitions are markdown files with frontmatter, discovered from the
//! project layer (`.agentkit/tackle/<kind>/`) overriding the user layer
//! (`~/.config/agentkit/tackle/<kind>/`). Frontmatter format is
//! marker-dispatched: `---` fences parse as YAML (the ecosystem convention,
//! so definitions lift in unchanged), `+++` fences parse as TOML.
//! Parsing is bounded by a size cap; validation errors name the file and,
//! via the deserialiser's message, the offending key.

use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Maximum definition file size, bounding parse work.
const DEFINITION_MAX_SIZE: usize = 256 * 1024;

const KIND_DIRS: DefinitionDirs = DefinitionDirs {
    personas: "personas",
    actors: "actors",
    prompts: "prompts",
};

#[derive(Debug, Clone, Copy)]
struct DefinitionDirs {
    personas: &'static str,
    actors: &'static str,
    prompts: &'static str,
}

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
}

impl Definitions {
    /// Registers the built-in defaults at the lowest precedence: any
    /// discovered definition with the same name replaces the built-in.
    pub fn with_builtins(mut self) -> Self {
        for (kind, name, raw) in crate::builtins::defaults() {
            let path = PathBuf::from(format!("<builtin>/{kind}-{name}.md"));
            match kind {
                "persona" => {
                    if let Ok(definition) = parse_definition::<PersonaMeta>(name, &path, raw) {
                        self.personas
                            .entry(definition.name.clone())
                            .or_insert(definition);
                    }
                }
                "actor" => {
                    if let Ok(definition) = parse_definition::<ActorMeta>(name, &path, raw) {
                        self.actors
                            .entry(definition.name.clone())
                            .or_insert(definition);
                    }
                }
                _ => {}
            }
        }
        self
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

/// Discovers definitions from the project layer (overriding) and the user
/// layer, then registers built-in defaults beneath both.
pub fn discover(
    user_dir: &Path,
    project_dir: Option<&Path>,
) -> Result<Definitions, DefinitionError> {
    let mut defs = Definitions::default();
    discover_kind(
        &mut defs.personas,
        user_dir,
        project_dir,
        KIND_DIRS.personas,
    )?;
    discover_kind(&mut defs.actors, user_dir, project_dir, KIND_DIRS.actors)?;
    discover_kind(&mut defs.prompts, user_dir, project_dir, KIND_DIRS.prompts)?;
    Ok(defs.with_builtins())
}

fn discover_kind<T: DeserializeOwned>(
    out: &mut BTreeMap<String, Definition<T>>,
    user_dir: &Path,
    project_dir: Option<&Path>,
    kind_dir: &str,
) -> Result<(), DefinitionError> {
    // User layer first: project entries overwrite by name.
    scan_dir(out, &user_dir.join(kind_dir))?;
    if let Some(project_dir) = project_dir {
        scan_dir(out, &project_dir.join(kind_dir))?;
    }
    Ok(())
}

fn scan_dir<T: DeserializeOwned>(
    out: &mut BTreeMap<String, Definition<T>>,
    dir: &Path,
) -> Result<(), DefinitionError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(DefinitionError::Io {
                path: dir.into(),
                source,
            })
        }
    };

    for entry in entries {
        let entry = entry.map_err(|source| DefinitionError::Io {
            path: dir.into(),
            source,
        })?;
        let path = entry.path();
        if !path.is_file() || path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("unicode file stem")
            .to_owned();
        let definition = parse_definition_file(&name, &path)?;
        out.insert(definition.name.clone(), definition);
    }
    Ok(())
}

fn parse_definition<T: DeserializeOwned>(
    name: &str,
    path: &Path,
    raw: &str,
) -> Result<Definition<T>, DefinitionError> {
    let (frontmatter_src, body) = split_frontmatter(path, raw)?;
    let frontmatter = parse_frontmatter(path, frontmatter_src)?;
    Ok(Definition {
        name: name.to_owned(),
        path: path.to_path_buf(),
        raw: raw.to_owned(),
        frontmatter,
        body: body.to_owned(),
    })
}

fn parse_definition_file<T: DeserializeOwned>(
    name: &str,
    path: &Path,
) -> Result<Definition<T>, DefinitionError> {
    let raw = std::fs::read_to_string(path).map_err(|source| DefinitionError::Io {
        path: path.into(),
        source,
    })?;
    if raw.len() > DEFINITION_MAX_SIZE {
        return Err(DefinitionError::TooLarge {
            path: path.into(),
            max: DEFINITION_MAX_SIZE,
        });
    }
    parse_definition(name, path, &raw)
}

/// Splits `raw` into (frontmatter source, body). The fence marker selects
/// the format: `---` for YAML, `+++` for TOML. The closing fence must
/// match the opening one.
fn split_frontmatter<'a>(path: &Path, raw: &'a str) -> Result<(&'a str, &'a str), DefinitionError> {
    const YAML_FENCE: &str = "---";
    const TOML_FENCE: &str = "+++";
    let (fence, rest) = if let Some(rest) = raw.strip_prefix(YAML_FENCE) {
        (YAML_FENCE, rest)
    } else if let Some(rest) = raw.strip_prefix(TOML_FENCE) {
        (TOML_FENCE, rest)
    } else {
        return Err(DefinitionError::MissingFrontmatter { path: path.into() });
    };

    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let close = format!("\n{fence}");
    let end = rest
        .find(&close)
        .ok_or_else(|| DefinitionError::UnterminatedFrontmatter { path: path.into() })?;
    let frontmatter = &rest[..end];
    let body = &rest[end + close.len()..];
    let body = body.strip_prefix('\n').unwrap_or(body);
    Ok((frontmatter, body))
}

fn parse_frontmatter<T: DeserializeOwned>(path: &Path, source: &str) -> Result<T, DefinitionError> {
    let fail = |message: String| DefinitionError::Frontmatter {
        path: path.into(),
        message,
    };
    if source.lines().any(|line| {
        let line = line.trim();
        !line.is_empty() && !line.starts_with('#') && line.contains(" = ")
    }) {
        // Line-oriented `key = value`: TOML. The `+++` fence selects it
        // explicitly, so this sniffer only disambiguates content parsed
        // from a `---` fence that cannot be YAML.
        toml::from_str(source).map_err(|err| fail(err.to_string()))
    } else {
        yaml_serde::from_str(source).map_err(|err| fail(err.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_def(dir: &Path, kind: &str, name: &str, content: &str) -> PathBuf {
        let kind_dir = dir.join(kind);
        std::fs::create_dir_all(&kind_dir).unwrap();
        let path = kind_dir.join(format!("{name}.md"));
        std::fs::write(&path, content).unwrap();
        path
    }

    const YAML_PERSONA: &str = "---\ndescription: Test persona\n---\nBody line.\n";
    const TOML_PERSONA: &str = "+++\ndescription = \"Test persona\"\n+++\nBody line.\n";

    #[test]
    fn yaml_frontmatter_parses() {
        let dir = tempfile::tempdir().unwrap();
        write_def(dir.path(), "personas", "test", YAML_PERSONA);
        let defs = discover(dir.path(), None).unwrap();
        let persona = defs.personas.get("test").unwrap();
        assert_eq!(
            persona.frontmatter.description.as_deref(),
            Some("Test persona")
        );
        assert_eq!(persona.body.trim(), "Body line.");
    }

    #[test]
    fn toml_frontmatter_parses_identically() {
        let dir = tempfile::tempdir().unwrap();
        write_def(dir.path(), "personas", "test", TOML_PERSONA);
        let defs = discover(dir.path(), None).unwrap();
        assert_eq!(
            defs.personas["test"].frontmatter.description.as_deref(),
            Some("Test persona")
        );
    }

    #[test]
    fn project_layer_overrides_user_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user");
        let project = dir.path().join("project");
        write_def(&user, "personas", "shared", YAML_PERSONA);
        write_def(
            &project,
            "personas",
            "shared",
            "+++\ndescription = \"project\"\n+++\n",
        );
        let defs = discover(&user, Some(&project)).unwrap();
        assert_eq!(
            defs.personas["shared"].frontmatter.description.as_deref(),
            Some("project")
        );
        assert!(defs.personas["shared"].path.starts_with(&project));
    }

    #[test]
    fn missing_frontmatter_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        write_def(dir.path(), "personas", "bare", "just a body\n");
        let err = discover(dir.path(), None).unwrap_err();
        assert!(err.to_string().contains("bare.md"), "{err}");
        assert!(err.to_string().contains("frontmatter"), "{err}");
    }

    #[test]
    fn actor_requires_persona() {
        let dir = tempfile::tempdir().unwrap();
        write_def(
            dir.path(),
            "actors",
            "orphan",
            "---\ndescription: no persona\n---\n",
        );
        let err = discover(dir.path(), None).unwrap_err();
        assert!(err.to_string().contains("persona"), "{err}");
    }

    #[test]
    fn prompt_defaults_to_user_invokable() {
        let dir = tempfile::tempdir().unwrap();
        write_def(
            dir.path(),
            "prompts",
            "cmd",
            "---\ndescription: A command\n---\nDo $1.\n",
        );
        let defs = discover(dir.path(), None).unwrap();
        let prompt = defs.prompts.get("cmd").unwrap();
        assert!(prompt.frontmatter.user_invokable);
        assert!(!prompt.frontmatter.model_invokable);
        assert!(!prompt.frontmatter.compaction);
    }

    #[test]
    fn oversized_definition_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let huge = format!(
            "---\ndescription: big\n---\n{}\n",
            "x".repeat(DEFINITION_MAX_SIZE + 1)
        );
        write_def(dir.path(), "personas", "huge", &huge);
        let err = discover(dir.path(), None).unwrap_err();
        assert!(err.to_string().contains("size cap"), "{err}");
    }

    #[test]
    fn empty_config_yields_builtin_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let defs = discover(dir.path(), None).unwrap();
        assert!(defs.personas.contains_key("default"));
        assert!(defs.actors.contains_key("default"));
        assert_eq!(defs.actors["default"].frontmatter.persona, "default");
    }

    #[test]
    fn discovered_definitions_replace_builtins() {
        let dir = tempfile::tempdir().unwrap();
        write_def(
            dir.path(),
            "actors",
            "default",
            "---\npersona: default\n---\nCustom.\n",
        );
        let defs = discover(dir.path(), None).unwrap();
        assert_eq!(defs.actors["default"].body.trim(), "Custom.");
    }
}
