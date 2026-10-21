//! Source resolution and vendoring: materialise selected definitions from
//! path and git sources into the repository tree.
//!
//! Vendoring writes the entry point and every resource. A flat definition
//! is materialised as a directory package with its kind's entry point and
//! no resources, so the tree has one shape regardless of source layout.

use super::manifest::SourceDecl;
use super::source::{self, GitCheckout};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum VendorError {
    #[error("{origin}: unknown definition kind `{kind}`")]
    UnknownKind { origin: String, kind: String },
    #[error("{origin}: {kind}/{name} is not provided by this source")]
    MissingDefinition {
        origin: String,
        kind: String,
        name: String,
    },
    #[error("{origin}: {kind}/{name} is both a flat file and a directory")]
    FlatDirectoryCollision {
        origin: String,
        kind: String,
        name: String,
    },
    #[error("{origin}: {kind}/{name}: missing {entry} entry point")]
    MissingEntryPoint {
        origin: String,
        kind: String,
        name: String,
        entry: &'static str,
    },
    #[error("{origin}: {kind}/{name}: unsupported file type at {path}")]
    UnsupportedFile {
        origin: String,
        kind: String,
        name: String,
        path: PathBuf,
    },
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to resolve source `{origin}`: {message}")]
    Resolve { origin: String, message: String },
}

/// A source checked out at an immutable revision. A git checkout's temp
/// tree is owned here and removed on drop.
pub struct SourceCheckout {
    pub name: String,
    /// The root containing the kind directories.
    pub root: PathBuf,
    pub commit: Option<String>,
    _checkout: Option<GitCheckout>,
}

impl SourceCheckout {
    /// Resolves a source declaration. Git sources are fetched at the
    /// declared ref; path sources resolve relative to the repository.
    pub fn resolve(project_dir: &Path, declaration: &SourceDecl) -> Result<Self, VendorError> {
        Self::resolve_at(project_dir, declaration, None)
    }

    /// Resolves a git source at an explicit immutable revision, ignoring
    /// the declared ref. Path sources are unaffected.
    pub fn resolve_at(
        project_dir: &Path,
        declaration: &SourceDecl,
        at: Option<&str>,
    ) -> Result<Self, VendorError> {
        let repo_root = project_dir
            .parent()
            .and_then(Path::parent)
            .unwrap_or(project_dir);
        if let Some(git) = &declaration.git {
            let checkout =
                source::resolve_git(&declaration.name, &git.url, at.or(git.git_ref.as_deref()))
                    .map_err(|err| VendorError::Resolve {
                        origin: declaration.name.clone(),
                        message: err.to_string(),
                    })?;
            let relative = git.path.as_deref().unwrap_or("");
            let root = checkout.path().join(relative);
            return Ok(Self {
                name: declaration.name.clone(),
                root,
                commit: Some(checkout.commit().to_owned()),
                _checkout: Some(checkout),
            });
        }
        if let Some(path) = &declaration.path {
            return Ok(Self {
                name: declaration.name.clone(),
                root: repo_root.join(&path.dir),
                commit: None,
                _checkout: None,
            });
        }
        Err(VendorError::Resolve {
            origin: declaration.name.clone(),
            message: "source has no spec".into(),
        })
    }
}

/// A definition written into the repository tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendoredDefinition {
    pub kind: String,
    pub name: String,
    pub path: PathBuf,
}

pub fn entry_point(kind: &str) -> Option<&'static str> {
    match kind {
        "personas" => Some("PERSONA.md"),
        "actors" => Some("ACTOR.md"),
        "prompts" => Some("PROMPT.md"),
        _ => None,
    }
}

/// A definition staged outside the tree, ready to be swapped into place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedDefinition {
    pub kind: String,
    pub name: String,
    /// The staged package directory.
    pub path: PathBuf,
}

/// Stages one selected definition under `staging_root` without touching
/// the repository tree.
pub fn stage_definition(
    staging_root: &Path,
    checkout: &SourceCheckout,
    kind: &str,
    name: &str,
) -> Result<StagedDefinition, VendorError> {
    let entry = entry_point(kind).ok_or_else(|| VendorError::UnknownKind {
        origin: checkout.name.clone(),
        kind: kind.to_owned(),
    })?;
    let kind_dir = checkout.root.join(kind);
    let flat = kind_dir.join(format!("{name}.md"));
    let package = kind_dir.join(name);
    let is_flat = flat.is_file();
    let is_package = package.is_dir();
    if is_flat && is_package {
        return Err(VendorError::FlatDirectoryCollision {
            origin: checkout.name.clone(),
            kind: kind.to_owned(),
            name: name.to_owned(),
        });
    }
    if !is_flat && !is_package {
        return Err(VendorError::MissingDefinition {
            origin: checkout.name.clone(),
            kind: kind.to_owned(),
            name: name.to_owned(),
        });
    }

    let staged = staging_root.join(kind).join(name);
    if staged.exists() {
        remove_dir_all(&staged)?;
    }
    std::fs::create_dir_all(&staged).map_err(|source| VendorError::Io {
        path: staged.clone(),
        source,
    })?;

    let result = (|| {
        if is_flat {
            std::fs::copy(&flat, staged.join(entry)).map_err(|source| VendorError::Io {
                path: flat.clone(),
                source,
            })?;
        } else {
            let source_entry = package.join(entry);
            if !source_entry.is_file() {
                return Err(VendorError::MissingEntryPoint {
                    origin: checkout.name.clone(),
                    kind: kind.to_owned(),
                    name: name.to_owned(),
                    entry,
                });
            }
            copy_tree(checkout, kind, name, &package, &staged)?;
        }
        Ok(())
    })();

    if let Err(error) = result {
        let _ = remove_dir_all(&staged);
        return Err(error);
    }

    Ok(StagedDefinition {
        kind: kind.to_owned(),
        name: name.to_owned(),
        path: staged,
    })
}

/// Vendors one selected definition. The destination is replaced only
/// after its replacement has been fully staged.
pub fn vendor_definition(
    project_dir: &Path,
    checkout: &SourceCheckout,
    kind: &str,
    name: &str,
) -> Result<VendoredDefinition, VendorError> {
    let staging_root = project_dir.join(".vendor-staging");
    std::fs::create_dir_all(&staging_root).map_err(|source| VendorError::Io {
        path: staging_root.clone(),
        source,
    })?;
    let staged = stage_definition(&staging_root, checkout, kind, name)?;
    let destination = project_dir.join(kind).join(name);
    std::fs::create_dir_all(project_dir.join(kind)).map_err(|source| VendorError::Io {
        path: project_dir.join(kind),
        source,
    })?;

    if destination.exists() {
        remove_dir_all(&destination)?;
    }
    std::fs::rename(&staged.path, &destination).map_err(|source| VendorError::Io {
        path: destination.clone(),
        source,
    })?;
    let _ = std::fs::remove_dir_all(&staging_root);

    Ok(VendoredDefinition {
        kind: kind.to_owned(),
        name: name.to_owned(),
        path: destination,
    })
}

fn copy_tree(
    checkout: &SourceCheckout,
    kind: &str,
    name: &str,
    from: &Path,
    to: &Path,
) -> Result<(), VendorError> {
    let entries = std::fs::read_dir(from).map_err(|source| VendorError::Io {
        path: from.into(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| VendorError::Io {
            path: from.into(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| VendorError::Io {
            path: path.clone(),
            source,
        })?;
        let destination = to.join(entry.file_name());
        if file_type.is_dir() {
            std::fs::create_dir(&destination).map_err(|source| VendorError::Io {
                path: destination.clone(),
                source,
            })?;
            copy_tree(checkout, kind, name, &path, &destination)?;
        } else if file_type.is_file() {
            std::fs::copy(&path, &destination).map_err(|source| VendorError::Io {
                path: path.clone(),
                source,
            })?;
        } else {
            return Err(VendorError::UnsupportedFile {
                origin: checkout.name.clone(),
                kind: kind.to_owned(),
                name: name.to_owned(),
                path,
            });
        }
    }
    Ok(())
}

fn remove_dir_all(path: &Path) -> Result<(), VendorError> {
    std::fs::remove_dir_all(path).map_err(|source| VendorError::Io {
        path: path.into(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join(".agentkit").join("tackle");
        std::fs::create_dir_all(&project).unwrap();
        (dir, project)
    }

    fn write_source(root: &Path) {
        let package = root.join("personas").join("reviewer");
        std::fs::create_dir_all(package.join("examples")).unwrap();
        std::fs::write(
            package.join("PERSONA.md"),
            "---\ndescription: Review\n---\nReview carefully.\n",
        )
        .unwrap();
        std::fs::write(package.join("checklist.txt"), "one\ntwo\n").unwrap();
        std::fs::write(package.join("examples").join("case.md"), "case\n").unwrap();
    }

    #[test]
    fn a_path_source_vendors_a_definition_with_resources() {
        let (dir, project) = project();
        let shared = dir.path().join("shared");
        write_source(&shared);
        let declaration = SourceDecl {
            name: "local".into(),
            kind: Some("path".into()),
            git: None,
            path: Some(super::super::manifest::PathSpec {
                dir: "shared".into(),
            }),
        };
        let checkout = SourceCheckout::resolve(&project, &declaration).unwrap();
        let vendored = vendor_definition(&project, &checkout, "personas", "reviewer").unwrap();

        assert_eq!(vendored.path, project.join("personas").join("reviewer"));
        assert_eq!(
            std::fs::read_to_string(vendored.path.join("PERSONA.md")).unwrap(),
            "---\ndescription: Review\n---\nReview carefully.\n"
        );
        assert_eq!(
            std::fs::read_to_string(vendored.path.join("checklist.txt")).unwrap(),
            "one\ntwo\n"
        );
        assert_eq!(
            std::fs::read_to_string(vendored.path.join("examples").join("case.md")).unwrap(),
            "case\n"
        );
    }

    #[test]
    fn a_git_source_vendors_the_same_tree() {
        let (dir, project) = project();
        let repository = dir.path().join("source");
        write_source(&repository);
        let repo = git2::Repository::init(&repository).unwrap();
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = git2::Signature::now("test", "test@example.com").unwrap();
        let commit = repo
            .commit(Some("HEAD"), &signature, &signature, "fixture", &tree, &[])
            .unwrap();
        repo.tag_lightweight("v1", &repo.find_object(commit, None).unwrap(), false)
            .unwrap();

        let declaration = SourceDecl {
            name: "team".into(),
            kind: Some("git".into()),
            git: Some(super::super::manifest::GitSpec {
                url: repository.to_str().unwrap().to_owned(),
                git_ref: Some("v1".into()),
                path: None,
            }),
            path: None,
        };
        let checkout = SourceCheckout::resolve(&project, &declaration).unwrap();
        let commit_string = commit.to_string();
        assert_eq!(checkout.commit.as_deref(), Some(commit_string.as_str()));
        let vendored = vendor_definition(&project, &checkout, "personas", "reviewer").unwrap();
        assert_eq!(
            std::fs::read_to_string(vendored.path.join("examples").join("case.md")).unwrap(),
            "case\n"
        );
    }

    #[test]
    fn a_flat_definition_is_vendored_as_a_package() {
        let (dir, project) = project();
        let shared = dir.path().join("shared");
        std::fs::create_dir_all(shared.join("prompts")).unwrap();
        std::fs::write(
            shared.join("prompts").join("triage.md"),
            "---\ndescription: Triage\n---\nTriage it.\n",
        )
        .unwrap();
        let declaration = SourceDecl {
            name: "local".into(),
            kind: Some("path".into()),
            git: None,
            path: Some(super::super::manifest::PathSpec {
                dir: "shared".into(),
            }),
        };
        let checkout = SourceCheckout::resolve(&project, &declaration).unwrap();
        let vendored = vendor_definition(&project, &checkout, "prompts", "triage").unwrap();
        assert!(vendored.path.join("PROMPT.md").is_file());
        assert_eq!(
            std::fs::read_to_string(vendored.path.join("PROMPT.md")).unwrap(),
            "---\ndescription: Triage\n---\nTriage it.\n"
        );
    }

    #[test]
    fn a_missing_definition_names_the_source_and_definition() {
        let (dir, project) = project();
        let shared = dir.path().join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        let declaration = SourceDecl {
            name: "local".into(),
            kind: Some("path".into()),
            git: None,
            path: Some(super::super::manifest::PathSpec {
                dir: "shared".into(),
            }),
        };
        let checkout = SourceCheckout::resolve(&project, &declaration).unwrap();
        let err = vendor_definition(&project, &checkout, "personas", "missing").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("local"), "{message}");
        assert!(message.contains("personas/missing"), "{message}");
    }
}
