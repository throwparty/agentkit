//! Definition loading: discovery across the user and project layers, marker-dispatched frontmatter parsing, and built-in registration.

use super::types::{Definition, DefinitionError, Definitions};
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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

/// The fixed entry-point filename per kind.
const ENTRY_POINTS: DefinitionDirs = DefinitionDirs {
    personas: "PERSONA.md",
    actors: "ACTOR.md",
    prompts: "PROMPT.md",
};

impl Definitions {
    /// Registers the built-in defaults at the lowest precedence: any
    /// discovered definition with the same name replaces the built-in,
    /// and the replacement is surfaced in `builtin_overrides`.
    pub fn with_builtins(mut self) -> Self {
        for (kind, name, raw) in crate::builtins::defaults() {
            let path = PathBuf::from(format!("<builtin>/{kind}-{name}.md"));
            match kind {
                "persona" => {
                    Self::register_builtin(
                        &mut self.personas,
                        &mut self.builtin_overrides,
                        kind,
                        name,
                        &path,
                        raw,
                    );
                }
                "actor" => {
                    Self::register_builtin(
                        &mut self.actors,
                        &mut self.builtin_overrides,
                        kind,
                        name,
                        &path,
                        raw,
                    );
                }
                "prompt" => {
                    Self::register_builtin(
                        &mut self.prompts,
                        &mut self.builtin_overrides,
                        kind,
                        name,
                        &path,
                        raw,
                    );
                }
                _ => {}
            }
        }
        self
    }

    fn register_builtin<T: DeserializeOwned>(
        map: &mut BTreeMap<String, Definition<T>>,
        overrides: &mut BTreeSet<String>,
        kind: &str,
        name: &str,
        path: &Path,
        raw: &str,
    ) {
        if map.contains_key(name) {
            overrides.insert(format!("{kind}:{name}"));
            return;
        }
        if let Ok(definition) = parse_definition::<T>(name, path, raw) {
            map.insert(definition.name.clone(), definition);
        }
    }
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
        ENTRY_POINTS.personas,
    )?;
    discover_kind(
        &mut defs.actors,
        user_dir,
        project_dir,
        KIND_DIRS.actors,
        ENTRY_POINTS.actors,
    )?;
    discover_kind(
        &mut defs.prompts,
        user_dir,
        project_dir,
        KIND_DIRS.prompts,
        ENTRY_POINTS.prompts,
    )?;
    Ok(defs.with_builtins())
}

fn discover_kind<T: DeserializeOwned>(
    out: &mut BTreeMap<String, Definition<T>>,
    user_dir: &Path,
    project_dir: Option<&Path>,
    kind_dir: &str,
    entry_name: &'static str,
) -> Result<(), DefinitionError> {
    // User layer first: project entries overwrite by name.
    scan_dir(out, &user_dir.join(kind_dir), entry_name, None)?;
    if let Some(project_dir) = project_dir {
        let root = project_dir
            .parent()
            .and_then(Path::parent)
            .unwrap_or(project_dir);
        scan_dir(out, &project_dir.join(kind_dir), entry_name, Some(root))?;
    }
    Ok(())
}

/// Scans one kind directory for both definition shapes: flat
/// `<name>.md` files and package directories `<name>/<ENTRY>`. A
/// directory without its kind's entry point is an error naming the path.
/// A flat file and a directory sharing a name is an error naming both.
fn scan_dir<T: DeserializeOwned>(
    out: &mut BTreeMap<String, Definition<T>>,
    dir: &Path,
    entry_name: &'static str,
    repo_root: Option<&Path>,
) -> Result<(), DefinitionError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(DefinitionError::Io {
                path: dir.into(),
                source,
            });
        }
    };

    let mut flats = BTreeMap::new();
    let mut directories = BTreeMap::new();
    for entry in entries {
        let entry = entry.map_err(|source| DefinitionError::Io {
            path: dir.into(),
            source,
        })?;
        let path = entry.path();
        let Some(name) = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        if path.is_dir() {
            directories.insert(name, path);
        } else if path.is_file() && path.extension().is_some_and(|ext| ext == "md") {
            let name = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .expect("unicode file stem")
                .to_owned();
            flats.insert(name, path);
        }
    }

    for (name, package_dir) in directories {
        if let Some(flat) = flats.remove(&name) {
            return Err(DefinitionError::FlatDirectoryCollision {
                name,
                flat,
                directory: package_dir,
            });
        }
        let definition = parse_definition_package(&name, &package_dir, entry_name, repo_root)?;
        out.insert(definition.name.clone(), definition);
    }

    for (name, path) in flats {
        let definition = parse_definition_file(&name, &path)?;
        out.insert(definition.name.clone(), definition);
    }
    Ok(())
}

/// Parses a directory definition: the kind's entry point plus resources.
/// The size cap applies to the package total, not just the entry point.
fn parse_definition_package<T: DeserializeOwned>(
    name: &str,
    package_dir: &Path,
    entry_name: &'static str,
    repo_root: Option<&Path>,
) -> Result<Definition<T>, DefinitionError> {
    let entry = package_dir.join(entry_name);
    if !entry.is_file() {
        return Err(DefinitionError::MissingEntryPoint {
            path: package_dir.to_path_buf(),
            entry: entry_name,
        });
    }
    let size = super::types::package_size(package_dir)?;
    if size > DEFINITION_MAX_SIZE {
        return Err(DefinitionError::TooLarge {
            path: entry,
            max: DEFINITION_MAX_SIZE,
        });
    }
    let raw = std::fs::read_to_string(&entry).map_err(|source| DefinitionError::Io {
        path: entry.clone(),
        source,
    })?;
    let mut definition = parse_definition::<T>(name, &entry, &raw)?;
    definition.directory = Some(
        repo_root
            .and_then(|root| package_dir.strip_prefix(root).ok())
            .unwrap_or(package_dir)
            .to_path_buf(),
    );
    definition.resources = super::types::collect_resources(package_dir, &entry)?;
    Ok(definition)
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
        directory: None,
        resources: Vec::new(),
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

#[cfg(test)]
mod package_tests {
    use super::*;

    fn write_package(
        root: &Path,
        kind: &str,
        name: &str,
        entry: &str,
        body: &str,
        resources: &[(&str, &str)],
    ) -> PathBuf {
        let package = root.join(kind).join(name);
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join(entry), body).unwrap();
        for (path, content) in resources {
            let path = package.join(path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, content).unwrap();
        }
        package
    }

    #[test]
    fn directory_definitions_of_every_kind_load() {
        let dir = tempfile::tempdir().unwrap();
        write_package(
            dir.path(),
            "personas",
            "styled",
            "PERSONA.md",
            "---\ndescription: Styled\n---\nPersona body.\n",
            &[("guide.md", "guide")],
        );
        write_package(
            dir.path(),
            "actors",
            "worker",
            "ACTOR.md",
            "---\npersona: styled\n---\n",
            &[("notes.txt", "notes")],
        );
        write_package(
            dir.path(),
            "prompts",
            "deploy",
            "PROMPT.md",
            "---\ndescription: Deploy\n---\nDeploy $1.\n",
            &[("script.rhai", "fn main() {}")],
        );

        let defs = discover(dir.path(), None).unwrap();
        let persona = defs.personas.get("styled").unwrap();
        assert_eq!(persona.body.trim(), "Persona body.");
        assert_eq!(persona.resources, vec!["guide.md"]);
        assert!(persona.directory.is_some());
        assert_eq!(
            defs.actors["worker"].frontmatter.persona,
            "styled".to_owned()
        );
        assert_eq!(defs.prompts["deploy"].resources, vec!["script.rhai"]);
    }

    #[test]
    fn missing_entry_point_names_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("personas").join("missing");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("resource.md"), "resource").unwrap();
        let err = discover(dir.path(), None).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("missing"), "{message}");
        assert!(message.contains("PERSONA.md"), "{message}");
    }

    #[test]
    fn misnamed_entry_point_names_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("personas").join("wrong");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("PROMPT.md"), "---\n---\nbody").unwrap();
        let err = discover(dir.path(), None).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("wrong"), "{message}");
        assert!(message.contains("PERSONA.md"), "{message}");
    }

    #[test]
    fn oversized_package_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        write_package(
            dir.path(),
            "personas",
            "huge",
            "PERSONA.md",
            "---\ndescription: big\n---\nbody",
            &[("resource.txt", &"x".repeat(DEFINITION_MAX_SIZE + 1))],
        );
        let err = discover(dir.path(), None).unwrap_err();
        assert!(err.to_string().contains("size cap"), "{err}");
    }

    #[test]
    fn directory_path_is_repository_relative() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user");
        let project = dir.path().join("project");
        write_package(
            &project,
            "personas",
            "styled",
            "PERSONA.md",
            "---\ndescription: Styled\n---\nbody",
            &[],
        );
        let defs = discover(&user, Some(&project)).unwrap();
        let directory = defs.personas["styled"].directory.as_deref().unwrap();
        assert!(
            directory.ends_with("project/personas/styled"),
            "{directory:?}"
        );
    }
}
