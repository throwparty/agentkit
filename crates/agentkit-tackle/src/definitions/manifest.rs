//! The definitions manifest: source declarations and the definitions
//! selected from each.
//!
//! The manifest is the control point for sourcing only. The loader never
//! reads it: loading is deterministic from the repository tree alone.
//! Sourcing is fail-closed, so every structural problem is a parse error
//! naming the offending source or definition.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The manifest filename under the tackle project directory.
pub const MANIFEST_FILE: &str = "definitions.toml";

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub source: Vec<SourceDecl>,
    #[serde(default)]
    pub definitions: BTreeMap<String, Selections>,
    #[serde(default)]
    pub extends: Vec<ExtendDecl>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SourceDecl {
    /// The name selections use to refer to this source.
    pub name: String,
    /// `git` or `path`. Required: there is no default.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub git: Option<GitSpec>,
    #[serde(default)]
    pub path: Option<PathSpec>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GitSpec {
    pub url: String,
    /// The requested ref (branch, tag, or commit). `HEAD` when absent.
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    /// A subdirectory of the source root holding the kind directories.
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PathSpec {
    /// A local directory holding the kind directories.
    pub dir: String,
}

/// A delegated manifest, resolved by the lock.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ExtendDecl {
    pub url: String,
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
}

/// The definitions selected from one source, keyed by kind.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Selections {
    #[serde(default)]
    pub personas: Vec<String>,
    #[serde(default)]
    pub actors: Vec<String>,
    #[serde(default)]
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
