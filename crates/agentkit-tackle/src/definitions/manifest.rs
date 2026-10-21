//! The definitions manifest: source declarations and the definitions
//! selected from each.
//!
//! The manifest is the control point for sourcing only. The loader never
//! reads it: loading is deterministic from the repository tree alone.
//! Sourcing is fail-closed, so every structural problem is a parse error
//! naming the offending source or definition.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::source;

/// The manifest filename under the tackle project directory.
pub const MANIFEST_FILE: &str = "definitions.toml";

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct Manifest {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source: Vec<SourceDecl>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub definitions: BTreeMap<String, Selections>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extends: Vec<ExtendDecl>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SourceDecl {
    /// The name selections use to refer to this source.
    pub name: String,
    /// `git` or `path`. Required: there is no default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathSpec>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct GitSpec {
    pub url: String,
    /// The requested ref (branch, tag, or commit). `HEAD` when absent.
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    /// A subdirectory of the source root holding the kind directories.
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct PathSpec {
    /// A local directory holding the kind directories.
    pub dir: String,
}

/// A delegated manifest, resolved by the lock.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ExtendDecl {
    pub url: String,
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
}

/// The definitions selected from one source, keyed by kind.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct Selections {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub personas: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<String>,
}

impl Selections {
    /// The selected names with their kind: `(kind, name)`.
    pub fn entries(&self) -> impl Iterator<Item = (&'static str, &str)> {
        [
            ("personas", self.personas.as_slice()),
            ("actors", self.actors.as_slice()),
            ("prompts", self.prompts.as_slice()),
        ]
        .into_iter()
        .flat_map(|(kind, names)| names.iter().map(move |name| (kind, name.as_str())))
    }

    pub fn is_empty(&self) -> bool {
        self.personas.is_empty() && self.actors.is_empty() && self.prompts.is_empty()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("{path}: {message}")]
    Invalid { path: PathBuf, message: String },
}

impl ManifestError {
    fn invalid(path: &Path, message: impl Into<String>) -> Self {
        Self::Invalid {
            path: path.to_path_buf(),
            message: message.into(),
        }
    }
}

/// The manifest path under a tackle project directory.
pub fn manifest_path(project_dir: &Path) -> PathBuf {
    project_dir.join(MANIFEST_FILE)
}

impl Manifest {
    /// Loads the manifest from `project_dir/definitions.toml`.
    pub fn load(project_dir: &Path) -> Result<Self, ManifestError> {
        let path = manifest_path(project_dir);
        let source = std::fs::read_to_string(&path).map_err(|source| ManifestError::Io {
            path: path.clone(),
            source,
        })?;
        Self::parse(&source, &path)
    }

    /// Parses and normalises a manifest. Every source must declare its
    /// kind, and a kind must name the matching spec table.
    pub fn parse(source: &str, path: &Path) -> Result<Self, ManifestError> {
        let mut manifest: Manifest =
            toml::from_str(source).map_err(|source| ManifestError::Parse {
                path: path.to_path_buf(),
                source,
            })?;
        for source in &mut manifest.source {
            source.normalise_git_url();
            source.validate_kind(path)?;
        }
        for extend in &mut manifest.extends {
            normalize_git_url(&mut extend.url, &mut extend.git_ref, &mut extend.path);
        }
        Ok(manifest)
    }

    /// The source declaration by name.
    pub fn source(&self, name: &str) -> Option<&SourceDecl> {
        self.source.iter().find(|source| source.name == name)
    }

    /// Resolves `[[extends]]` transitively: fetches each delegated
    /// manifest at its ref, union-merges its sources and selections with
    /// this one, and rejects duplicate names anywhere in the resolved set.
    /// No override is permitted.
    pub fn resolve(&self, project_dir: &Path) -> Result<Manifest, ManifestError> {
        let path = manifest_path(project_dir);
        let mut visited = BTreeSet::new();
        let mut resolved = Manifest::default();
        self.collect(&path, &mut visited, &mut resolved)?;
        resolved.reject_duplicate_names(&path)?;
        Ok(resolved)
    }

    fn collect(
        &self,
        path: &Path,
        visited: &mut BTreeSet<String>,
        out: &mut Manifest,
    ) -> Result<(), ManifestError> {
        for extend in &self.extends {
            let key = format!(
                "{}@{}#{}",
                extend.url,
                extend.git_ref.as_deref().unwrap_or("HEAD"),
                extend.path.as_deref().unwrap_or("")
            );
            if !visited.insert(key.clone()) {
                return Err(ManifestError::invalid(
                    path,
                    format!("manifest delegation cycle detected at `{key}`"),
                ));
            }
            let checkout = source::resolve_git("extends", &extend.url, extend.git_ref.as_deref())
                .map_err(|err| ManifestError::invalid(path, err.to_string()))?;
            let subdirectory = extend.path.as_deref().unwrap_or("");
            let delegated_path = checkout.path().join(subdirectory).join(MANIFEST_FILE);
            let source =
                std::fs::read_to_string(&delegated_path).map_err(|source| ManifestError::Io {
                    path: delegated_path.clone(),
                    source,
                })?;
            let delegated = Manifest::parse(&source, &delegated_path)?;
            delegated.collect(&delegated_path, visited, out)?;
        }
        self.merge_into(out, path)
    }

    fn merge_into(&self, out: &mut Manifest, path: &Path) -> Result<(), ManifestError> {
        for source in &self.source {
            if out
                .source
                .iter()
                .any(|existing| existing.name == source.name)
            {
                return Err(ManifestError::invalid(
                    path,
                    format!(
                        "source `{}` is declared twice in the resolved set",
                        source.name
                    ),
                ));
            }
            out.source.push(source.clone());
        }
        for (name, selections) in &self.definitions {
            let entry = out.definitions.entry(name.clone()).or_default();
            entry.personas.extend(selections.personas.iter().cloned());
            entry.actors.extend(selections.actors.iter().cloned());
            entry.prompts.extend(selections.prompts.iter().cloned());
        }
        Ok(())
    }

    /// Unique names across the resolved set: one name, one definition.
    fn reject_duplicate_names(&self, path: &Path) -> Result<(), ManifestError> {
        let mut names: BTreeMap<(&str, &str), &str> = BTreeMap::new();
        for (source_name, selections) in &self.definitions {
            for (kind, name) in selections.entries() {
                if let Some(previous) = names.insert((kind, name), source_name) {
                    return Err(ManifestError::invalid(
                        path,
                        format!(
                            "definition `{kind}/{name}` is selected by both `{previous}` and `{source_name}`"
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Enforces the manifest invariants: unique source and local
    /// definition names, selections that name a declared source, and
    /// source and definition paths confined to their roots.
    pub fn validate(&self, project_dir: &Path) -> Result<(), ManifestError> {
        let path = manifest_path(project_dir);
        let repo_root = project_dir
            .parent()
            .and_then(Path::parent)
            .unwrap_or(project_dir);

        let mut sources = BTreeSet::new();
        for source in &self.source {
            if !sources.insert(source.name.as_str()) {
                return Err(ManifestError::invalid(
                    &path,
                    format!("source `{}` is declared twice", source.name),
                ));
            }
        }

        for (source_name, selections) in &self.definitions {
            if self.source(source_name).is_none() {
                return Err(ManifestError::invalid(
                    &path,
                    format!("selections name unknown source `{source_name}`"),
                ));
            }
            for (kind, name) in selections.entries() {
                if !valid_definition_name(name) {
                    return Err(ManifestError::invalid(
                        &path,
                        format!("source `{source_name}` selects an invalid {kind} name `{name}`"),
                    ));
                }
            }
        }

        for source in &self.source {
            if let Some(git) = &source.git {
                if let Some(subdirectory) = git.path.as_deref() {
                    if !confined_relative(subdirectory) {
                        return Err(ManifestError::invalid(
                            &path,
                            format!(
                                "source `{}` path `{subdirectory}` escapes the source root",
                                source.name
                            ),
                        ));
                    }
                }
            }
            if let Some(spec) = &source.path {
                if !confined_to(repo_root, &spec.dir) {
                    return Err(ManifestError::invalid(
                        &path,
                        format!(
                            "source `{}` directory `{}` escapes the repository",
                            source.name, spec.dir
                        ),
                    ));
                }
            }
        }

        self.reject_duplicate_names(&path)
    }
}

/// A definition name is one ordinary path component: it cannot escape the
/// kind directory or name a file in it.
fn valid_definition_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\\')
}

/// A source-relative path must stay under the source root.
fn confined_relative(path: &str) -> bool {
    let path = Path::new(path);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

/// A local source directory must stay inside the repository.
fn confined_to(_root: &Path, dir: &str) -> bool {
    let dir = Path::new(dir);
    if dir.is_absolute() {
        return false;
    }
    !dir.components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
}

impl SourceDecl {
    fn normalise_git_url(&mut self) {
        if let Some(git) = &mut self.git {
            normalize_git_url(&mut git.url, &mut git.git_ref, &mut git.path);
        }
    }

    fn validate_kind(&self, path: &Path) -> Result<(), ManifestError> {
        let Some(kind) = self.kind.as_deref() else {
            return Err(ManifestError::invalid(
                path,
                format!("source `{}` must declare a kind", self.name),
            ));
        };
        match kind {
            "git" if self.git.is_none() => Err(ManifestError::invalid(
                path,
                format!(
                    "source `{}` has kind git but no [source.git] table",
                    self.name
                ),
            )),
            "path" if self.path.is_none() => Err(ManifestError::invalid(
                path,
                format!(
                    "source `{}` has kind path but no [source.path] table",
                    self.name
                ),
            )),
            "git" | "path" => Ok(()),
            other => Err(ManifestError::invalid(
                path,
                format!("source `{}` has unknown kind `{other}`", self.name),
            )),
        }
    }
}

/// The URL form accepted by `url`: the PEP 508 `git+` prefix, an optional
/// `@ref` after the last path segment, and an optional
/// `#subdirectory=` fragment. Explicit fields win.
fn normalize_git_url(url: &mut String, git_ref: &mut Option<String>, path: &mut Option<String>) {
    let Some(rest) = url.strip_prefix("git+") else {
        return;
    };
    let (rest, subdirectory) = match rest.split_once("#subdirectory=") {
        Some((rest, subdirectory)) => (rest, Some(subdirectory.to_owned())),
        None => (rest, None),
    };
    // A ref follows the last path segment (`repo@v1`). An `@` before the
    // last slash belongs to the userinfo (`git@host`).
    let reference = rest
        .rfind('/')
        .and_then(|slash| rest[slash..].find('@').map(|offset| slash + offset))
        .map(|at| (rest[..at].to_owned(), rest[at + 1..].to_owned()));
    let (base, reference) = match reference {
        Some((base, reference)) => (base, Some(reference)),
        None => (rest.to_owned(), None),
    };
    *url = base;
    if git_ref.is_none() {
        *git_ref = reference;
    }
    if path.is_none() {
        *path = subdirectory;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> Result<Manifest, ManifestError> {
        Manifest::parse(source, Path::new(".agentkit/tackle/definitions.toml"))
    }

    #[test]
    fn both_source_kinds_parse() {
        let manifest = parse(
            r#"
[[source]]
name = "team"
kind = "git"

[source.git]
url = "https://example.com/team.git"
ref = "v1"
path = "defs"

[[source]]
name = "local"
kind = "path"

[source.path]
dir = "shared"

[definitions.team]
personas = ["reviewer"]
prompts = ["deploy"]

[definitions.local]
actors = ["worker"]
"#,
        )
        .unwrap();

        assert_eq!(manifest.source.len(), 2);
        let team = manifest.source("team").unwrap();
        assert_eq!(team.kind.as_deref(), Some("git"));
        assert_eq!(team.git.as_ref().unwrap().git_ref.as_deref(), Some("v1"));
        assert_eq!(manifest.definitions["team"].personas, ["reviewer"]);
        assert_eq!(manifest.definitions["local"].actors, ["worker"]);
        let selections: Vec<_> = manifest.definitions["team"].entries().collect();
        assert!(selections.contains(&("personas", "reviewer")));
        assert!(selections.contains(&("prompts", "deploy")));
    }

    #[test]
    fn the_pep_508_git_form_parses_into_url_ref_and_subdirectory() {
        let manifest = parse(
            r#"
[[source]]
name = "team"
kind = "git"

[source.git]
url = "git+ssh://git@example.com/team.git@v1#subdirectory=defs"
"#,
        )
        .unwrap();
        let git = manifest.source("team").unwrap().git.as_ref().unwrap();
        assert_eq!(git.url, "ssh://git@example.com/team.git");
        assert_eq!(git.git_ref.as_deref(), Some("v1"));
        assert_eq!(git.path.as_deref(), Some("defs"));
    }

    #[test]
    fn the_git_form_without_a_ref_keeps_the_userinfo() {
        let manifest = parse(
            r#"
[[source]]
name = "team"
kind = "git"

[source.git]
url = "git+https://example.com/team.git"
"#,
        )
        .unwrap();
        let git = manifest.source("team").unwrap().git.as_ref().unwrap();
        assert_eq!(git.url, "https://example.com/team.git");
        assert!(git.git_ref.is_none());
    }

    #[test]
    fn a_source_without_kind_is_a_parse_error_naming_the_source() {
        let err = parse(
            r#"
[[source]]
name = "team"

[source.git]
url = "https://example.com/team.git"
"#,
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("team"), "{message}");
        assert!(message.contains("kind"), "{message}");
    }

    fn validate(source: &str) -> Result<(), ManifestError> {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join(".agentkit").join("tackle");
        std::fs::create_dir_all(&project).unwrap();
        let manifest = Manifest::parse(source, &manifest_path(&project))?;
        manifest.validate(&project)
    }

    #[test]
    fn a_duplicate_name_across_sources_errors() {
        let err = validate(
            r#"
[[source]]
name = "a"
kind = "git"

[source.git]
url = "https://example.com/a.git"

[[source]]
name = "b"
kind = "git"

[source.git]
url = "https://example.com/b.git"

[definitions.a]
personas = ["shared"]

[definitions.b]
personas = ["shared"]
"#,
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("shared"), "{message}");
        assert!(message.contains('a'), "{message}");
        assert!(message.contains('b'), "{message}");
    }

    #[test]
    fn a_source_path_escaping_the_source_root_errors() {
        let err = validate(
            r#"
[[source]]
name = "team"
kind = "git"

[source.git]
url = "https://example.com/team.git"
path = "../outside"
"#,
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("team"), "{message}");
        assert!(message.contains("escapes"), "{message}");
    }

    #[test]
    fn a_definition_path_escaping_the_repository_errors() {
        let err = validate(
            r#"
[[source]]
name = "local"
kind = "path"

[source.path]
dir = "../outside"
"#,
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("local"), "{message}");
        assert!(message.contains("escapes the repository"), "{message}");
    }

    #[test]
    fn an_invalid_definition_name_errors() {
        let err = validate(
            r#"
[[source]]
name = "local"
kind = "path"

[source.path]
dir = "shared"

[definitions.local]
personas = ["../escape"]
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("../escape"), "{err}");
    }

    #[test]
    fn a_valid_manifest_validates() {
        validate(
            r#"
[[source]]
name = "local"
kind = "path"

[source.path]
dir = "shared"

[definitions.local]
personas = ["reviewer"]
"#,
        )
        .unwrap();
    }

    fn git_repo(manifest: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        std::fs::write(dir.path().join(MANIFEST_FILE), manifest).unwrap();
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = git2::Signature::now("test", "test@example.com").unwrap();
        repo.commit(Some("HEAD"), &signature, &signature, "fixture", &tree, &[])
            .unwrap();
        let url = dir.path().to_str().unwrap().to_owned();
        (dir, url)
    }

    fn local_manifest(extends: &str, extra: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join(".agentkit").join("tackle");
        std::fs::create_dir_all(&project).unwrap();
        let manifest = format!(
            r#"
[[source]]
name = "local"
kind = "path"

[source.path]
dir = "shared"

[[extends]]
url = "{extends}"
{extra}

[definitions.local]
personas = ["local-reviewer"]
"#
        );
        std::fs::write(project.join(MANIFEST_FILE), manifest).unwrap();
        let project_string = project.to_str().unwrap().to_owned();
        (dir, project_string)
    }

    #[test]
    fn a_delegated_manifest_contributes_its_definitions() {
        let (_delegated_dir, delegated) = git_repo(
            r#"
[[source]]
name = "team"
kind = "path"

[source.path]
dir = "shared"

[definitions.team]
personas = ["reviewer"]
"#,
        );
        let (_dir, project) = local_manifest(&delegated, "");
        let manifest = Manifest::load(Path::new(&project)).unwrap();
        let resolved = manifest.resolve(Path::new(&project)).unwrap();
        let team = resolved.definitions.get("team").unwrap();
        assert_eq!(team.personas, vec!["reviewer"]);
        assert!(resolved.definitions.contains_key("local"));
        assert!(resolved.source.iter().any(|source| source.name == "team"));
    }

    #[test]
    fn a_name_contributed_by_a_delegated_manifest_cannot_be_overridden() {
        let (_delegated_dir, delegated) = git_repo(
            r#"
[[source]]
name = "team"
kind = "path"

[source.path]
dir = "shared"

[definitions.team]
personas = ["local-reviewer"]
"#,
        );
        let (_dir, project) = local_manifest(&delegated, "");
        let manifest = Manifest::load(Path::new(&project)).unwrap();
        let err = manifest.resolve(Path::new(&project)).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("local-reviewer"), "{message}");
        assert!(message.contains("local"), "{message}");
        assert!(message.contains("team"), "{message}");
    }

    #[test]
    fn nested_delegation_contributes_every_level() {
        let (_inner_dir, inner) = git_repo(
            r#"
[[source]]
name = "inner"
kind = "path"

[source.path]
dir = "shared"

[definitions.inner]
prompts = ["triage"]
"#,
        );
        let (_middle_dir, middle) = git_repo(&format!(
            r#"
[[source]]
name = "middle"
kind = "path"

[source.path]
dir = "shared"

[[extends]]
url = "{inner}"

[definitions.middle]
actors = ["worker"]
"#
        ));
        let (_dir, project) = local_manifest(&middle, "");
        let manifest = Manifest::load(Path::new(&project)).unwrap();
        let resolved = manifest.resolve(Path::new(&project)).unwrap();
        assert!(resolved.definitions.contains_key("inner"));
        assert!(resolved.definitions.contains_key("middle"));
        assert!(resolved.definitions.contains_key("local"));
    }

    #[test]
    fn a_delegation_cycle_errors() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        let url = dir.path().to_str().unwrap().to_owned();
        std::fs::write(
            dir.path().join(MANIFEST_FILE),
            format!(
                r#"
[[extends]]
url = "{url}"
"#
            ),
        )
        .unwrap();
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = git2::Signature::now("test", "test@example.com").unwrap();
        repo.commit(Some("HEAD"), &signature, &signature, "cycle", &tree, &[])
            .unwrap();

        let project = dir.path().join(".agentkit").join("tackle");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join(MANIFEST_FILE),
            format!(
                r#"
[[extends]]
url = "{url}"
"#
            ),
        )
        .unwrap();
        let manifest = Manifest::load(&project).unwrap();
        let err = manifest.resolve(&project).unwrap_err();
        assert!(err.to_string().contains("cycle"), "{err}");
    }

    #[test]
    fn extended_declarations_normalise_git_urls() {
        let manifest = parse(
            r#"
[[extends]]
url = "git+https://example.com/commons.git@v2#subdirectory=defs"
"#,
        )
        .unwrap();
        let extend = &manifest.extends[0];
        assert_eq!(extend.url, "https://example.com/commons.git");
        assert_eq!(extend.git_ref.as_deref(), Some("v2"));
        assert_eq!(extend.path.as_deref(), Some("defs"));
    }
}
