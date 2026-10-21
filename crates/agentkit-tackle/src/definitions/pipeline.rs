//! The vendoring pipeline: `get` reproduces the locked revisions, `update`
//! re-resolves declared refs and rewrites the lock.
//!
//! Both are the only network operations. Everything is staged before the
//! tree is touched, and every applied move is rolled back if a later one
//! fails, so a failed run leaves the tree and lock unchanged.

use super::lock::{self, Lock, LockedDefinition, LockedSource};
use super::manifest::{Manifest, ManifestError, SourceDecl};
use super::vendor::{self, SourceCheckout, StagedDefinition, VendorError};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error(transparent)]
    Lock(#[from] lock::LockError),
    #[error(transparent)]
    Vendor(#[from] VendorError),
    #[error("no lock records source `{name}`; run `tackle definitions update`")]
    UnlockedSource { name: String },
    #[error("no lock records definition `{kind}/{name}`; run `tackle definitions update`")]
    UnlockedDefinition { kind: String, name: String },
    #[error("source `{name}` changed since the lock was written; run `tackle definitions update`")]
    SourceChanged { name: String },
    #[error(
        "definition `{kind}/{name}` has drifted from the lock; run `tackle definitions update`"
    )]
    Drift {
        kind: String,
        name: String,
        expected: String,
        actual: String,
    },
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vendored {
    pub kind: String,
    pub name: String,
    pub source: String,
    pub path: PathBuf,
    pub hash: String,
}

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new() -> Result<Self, PipelineError> {
        let path = std::env::temp_dir().join(format!("tackle-vendor-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).map_err(|source| PipelineError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(Self { path })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[derive(Clone)]
struct Selection {
    source: String,
    kind: String,
    name: String,
}

impl Selection {
    fn key(&self) -> String {
        format!("{}/{}", self.kind, self.name)
    }
}

fn selections(manifest: &Manifest) -> Vec<Selection> {
    let mut out = Vec::new();
    for (source, selected) in &manifest.definitions {
        for (kind, name) in selected.entries() {
            out.push(Selection {
                source: source.clone(),
                kind: kind.to_owned(),
                name: name.to_owned(),
            });
        }
    }
    out.sort_by(|left, right| (&left.kind, &left.name).cmp(&(&right.kind, &right.name)));
    out
}

fn load_manifest(project_dir: &Path) -> Result<Manifest, PipelineError> {
    let manifest = Manifest::load(project_dir)?;
    manifest.validate(project_dir)?;
    Ok(manifest.resolve(project_dir)?)
}

/// Checks out every declared source, at the declared ref or, if
/// `locked`, at the immutable revision the lock records.
fn checkouts(
    project_dir: &Path,
    manifest: &Manifest,
    locked: Option<&Lock>,
) -> Result<BTreeMap<String, SourceCheckout>, PipelineError> {
    let mut out = BTreeMap::new();
    for declaration in &manifest.source {
        let checkout = match locked {
            None => SourceCheckout::resolve(project_dir, declaration)?,
            Some(lock) => {
                let locked = lock.sources.get(&declaration.name).ok_or_else(|| {
                    PipelineError::UnlockedSource {
                        name: declaration.name.clone(),
                    }
                })?;
                verify_source(declaration, locked)?;
                if declaration.git.is_some() {
                    SourceCheckout::resolve_at(project_dir, declaration, Some(&locked.commit))?
                } else {
                    SourceCheckout::resolve(project_dir, declaration)?
                }
            }
        };
        out.insert(declaration.name.clone(), checkout);
    }
    Ok(out)
}

fn verify_source(declaration: &SourceDecl, locked: &LockedSource) -> Result<(), PipelineError> {
    let changed = if let Some(git) = &declaration.git {
        git.url != locked.url || locked.commit.is_empty()
    } else if let Some(path) = &declaration.path {
        path.dir != locked.url
    } else {
        true
    };
    if changed {
        return Err(PipelineError::SourceChanged {
            name: declaration.name.clone(),
        });
    }
    Ok(())
}

pub fn get(project_dir: &Path) -> Result<Vec<Vendored>, PipelineError> {
    let manifest = load_manifest(project_dir)?;
    let lock = Lock::load(project_dir)?;
    let checkouts = checkouts(project_dir, &manifest, Some(&lock))?;
    let scratch = Scratch::new()?;

    let mut staged = Vec::new();
    for selection in selections(&manifest) {
        let key = selection.key();
        let locked =
            lock.definitions
                .get(&key)
                .ok_or_else(|| PipelineError::UnlockedDefinition {
                    kind: selection.kind.clone(),
                    name: selection.name.clone(),
                })?;
        if locked.source != selection.source {
            return Err(PipelineError::SourceChanged {
                name: selection.source.clone(),
            });
        }
        let checkout = checkouts.get(&selection.source).expect("checked out");
        let definition =
            vendor::stage_definition(&scratch.path, checkout, &selection.kind, &selection.name)?;
        let actual = lock::hash_package(&definition.path)?;
        if actual != locked.hash {
            return Err(PipelineError::Drift {
                kind: selection.kind.clone(),
                name: selection.name.clone(),
                expected: locked.hash.clone(),
                actual,
            });
        }
        staged.push((definition, selection, actual));
    }

    apply(project_dir, &staged, None)?;
    Ok(vendored(project_dir, staged))
}

pub fn update(project_dir: &Path) -> Result<Vec<Vendored>, PipelineError> {
    let manifest = load_manifest(project_dir)?;
    let checkouts = checkouts(project_dir, &manifest, None)?;
    let scratch = Scratch::new()?;

    let mut staged = Vec::new();
    for selection in selections(&manifest) {
        let checkout = checkouts.get(&selection.source).expect("checked out");
        let definition =
            vendor::stage_definition(&scratch.path, checkout, &selection.kind, &selection.name)?;
        let hash = lock::hash_package(&definition.path)?;
        staged.push((definition, selection, hash));
    }

    let mut lock = Lock::new();
    for declaration in &manifest.source {
        let checkout = checkouts.get(&declaration.name).expect("checked out");
        let (url, revision) = if let Some(git) = &declaration.git {
            (git.url.clone(), git.git_ref.clone())
        } else {
            (
                declaration
                    .path
                    .as_ref()
                    .map(|path| path.dir.clone())
                    .unwrap_or_default(),
                None,
            )
        };
        lock.sources.insert(
            declaration.name.clone(),
            LockedSource {
                commit: checkout.commit.clone().unwrap_or_default(),
                url,
                revision,
            },
        );
    }
    for (_, selection, hash) in &staged {
        lock.definitions.insert(
            selection.key(),
            LockedDefinition {
                source: selection.source.clone(),
                hash: hash.clone(),
            },
        );
    }

    apply(project_dir, &staged, Some(&lock))?;
    Ok(vendored(project_dir, staged))
}

fn vendored(
    project_dir: &Path,
    staged: Vec<(StagedDefinition, Selection, String)>,
) -> Vec<Vendored> {
    staged
        .into_iter()
        .map(|(definition, selection, hash)| Vendored {
            path: project_dir.join(&definition.kind).join(&definition.name),
            kind: definition.kind,
            name: definition.name,
            source: selection.source,
            hash,
        })
        .collect()
}

/// Applies staged definitions and, last, the lock. Any failure rolls the
/// tree back to its previous state.
fn apply(
    project_dir: &Path,
    staged: &[(StagedDefinition, Selection, String)],
    lock: Option<&Lock>,
) -> Result<(), PipelineError> {
    let lock_path = lock::lock_path(project_dir);
    let temporary_lock = lock_path.with_extension("lock.tmp");
    if let Some(lock) = lock {
        let body = toml::to_string_pretty(lock).map_err(|err| {
            PipelineError::Lock(lock::LockError::Parse {
                path: lock_path.clone(),
                message: err.to_string(),
            })
        })?;
        std::fs::write(&temporary_lock, body).map_err(|source| PipelineError::Io {
            path: temporary_lock.clone(),
            source,
        })?;
    }

    let scratch = Scratch::new()?;
    let mut applied: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    for (definition, _, _) in staged {
        let destination = project_dir.join(&definition.kind).join(&definition.name);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|source| PipelineError::Io {
                path: parent.into(),
                source,
            })?;
        }
        let backup = scratch.path.join(&definition.kind).join(&definition.name);
        let previous = if destination.exists() {
            if let Some(parent) = backup.parent() {
                std::fs::create_dir_all(parent).map_err(|source| PipelineError::Io {
                    path: parent.into(),
                    source,
                })?;
            }
            std::fs::rename(&destination, &backup).map_err(|source| PipelineError::Io {
                path: destination.clone(),
                source,
            })?;
            Some(backup)
        } else {
            None
        };
        if let Err(source) = std::fs::rename(&definition.path, &destination) {
            if let Some(previous) = &previous {
                let _ = std::fs::rename(previous, &destination);
            }
            rollback(&applied);
            let _ = std::fs::remove_file(&temporary_lock);
            return Err(PipelineError::Io {
                path: destination,
                source,
            });
        }
        applied.push((destination, previous));
    }

    if lock.is_some() {
        if let Err(source) = std::fs::rename(&temporary_lock, &lock_path) {
            rollback(&applied);
            let _ = std::fs::remove_file(&temporary_lock);
            return Err(PipelineError::Io {
                path: lock_path,
                source,
            });
        }
    }
    Ok(())
}

fn rollback(applied: &[(PathBuf, Option<PathBuf>)]) {
    for (destination, backup) in applied.iter().rev() {
        let _ = std::fs::remove_dir_all(destination);
        if let Some(backup) = backup {
            let _ = std::fs::rename(backup, destination);
        }
    }
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

    fn write_source(root: &Path, body: &str) {
        let package = root.join("personas").join("reviewer");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("PERSONA.md"),
            format!("---\ndescription: Review\n---\n{body}\n"),
        )
        .unwrap();
        std::fs::write(package.join("checklist.txt"), "check\n").unwrap();
    }

    fn write_manifest(project: &Path, url: &str, revision: Option<&str>) {
        let revision = revision
            .map(|revision| format!("git_ref = \"{revision}\"\n"))
            .unwrap_or_default();
        std::fs::write(
            project.join("definitions.toml"),
            format!(
                r#"
[[source]]
name = "team"
kind = "git"

[source.git]
url = "{url}"
{revision}

[definitions.team]
personas = ["reviewer"]
"#
            ),
        )
        .unwrap();
    }

    fn commit(repository: &Path, body: &str, branch: &str) -> String {
        std::fs::create_dir_all(repository.join("personas").join("reviewer")).unwrap();
        write_source(repository, body);
        let repo = git2::Repository::open(repository).unwrap();
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = git2::Signature::now("test", "test@example.com").unwrap();
        let reference = format!("refs/heads/{branch}");
        let parents = repo
            .find_reference(&reference)
            .ok()
            .and_then(|reference| reference.peel_to_commit().ok())
            .into_iter()
            .collect::<Vec<_>>();
        let parents = parents.iter().collect::<Vec<_>>();
        repo.commit(
            Some(&reference),
            &signature,
            &signature,
            body,
            &tree,
            &parents,
        )
        .unwrap()
        .to_string()
    }

    fn repository(dir: &Path) -> git2::Repository {
        let repo = git2::Repository::init(dir).unwrap();
        let signature = git2::Signature::now("test", "test@example.com").unwrap();
        let tree = {
            let mut index = repo.index().unwrap();
            let id = index.write_tree().unwrap();
            repo.find_tree(id).unwrap()
        };
        repo.commit(
            Some("refs/heads/main"),
            &signature,
            &signature,
            "root",
            &tree,
            &[],
        )
        .unwrap();
        drop(tree);
        repo.set_head("refs/heads/main").unwrap();
        repo
    }

    fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    walk(root, &path, out);
                } else {
                    let relative = path.strip_prefix(root).unwrap().to_string_lossy();
                    out.insert(relative.to_string(), std::fs::read(&path).unwrap());
                }
            }
        }
        walk(root, root, &mut out);
        out
    }

    #[test]
    fn get_reproduces_locked_revisions_and_update_advances_them() {
        let (dir, project) = project();
        let source = dir.path().join("source");
        repository(&source);
        let first = commit(&source, "first", "main");
        let url = source.to_str().unwrap().to_owned();
        write_manifest(&project, &url, Some("main"));

        let vendored = update(&project).unwrap();
        assert_eq!(vendored.len(), 1);
        assert_eq!(
            std::fs::read_to_string(project.join("personas/reviewer/PERSONA.md")).unwrap(),
            "---\ndescription: Review\n---\nfirst\n"
        );
        assert_eq!(Lock::load(&project).unwrap().sources["team"].commit, first);

        let second = commit(&source, "second", "main");
        // The branch advanced, but get must reproduce the locked commit.
        get(&project).unwrap();
        assert_eq!(
            std::fs::read_to_string(project.join("personas/reviewer/PERSONA.md")).unwrap(),
            "---\ndescription: Review\n---\nfirst\n"
        );
        assert_eq!(Lock::load(&project).unwrap().sources["team"].commit, first);

        update(&project).unwrap();
        assert_eq!(
            std::fs::read_to_string(project.join("personas/reviewer/PERSONA.md")).unwrap(),
            "---\ndescription: Review\n---\nsecond\n"
        );
        assert_eq!(Lock::load(&project).unwrap().sources["team"].commit, second);
    }

    #[test]
    fn an_unreachable_source_leaves_the_tree_and_lock_byte_identical() {
        let (dir, project) = project();
        let source = dir.path().join("source");
        repository(&source);
        commit(&source, "body", "main");
        let url = source.to_str().unwrap().to_owned();
        write_manifest(&project, &url, Some("main"));
        update(&project).unwrap();

        let before_tree = snapshot(&project.join("personas"));
        let before_lock = std::fs::read(project.join("definitions.lock")).unwrap();

        write_manifest(&project, "/nonexistent/source", Some("main"));
        assert!(update(&project).is_err());

        assert_eq!(snapshot(&project.join("personas")), before_tree);
        assert_eq!(
            std::fs::read(project.join("definitions.lock")).unwrap(),
            before_lock
        );
    }

    #[test]
    fn get_without_a_lock_names_update() {
        let (dir, project) = project();
        let source = dir.path().join("source");
        repository(&source);
        commit(&source, "body", "main");
        write_manifest(&project, &source.to_str().unwrap().to_owned(), Some("main"));
        let err = get(&project).unwrap_err();
        assert!(err.to_string().contains("definitions get"), "{err}");
    }
}
