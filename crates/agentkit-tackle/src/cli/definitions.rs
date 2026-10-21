//! The `tackle definitions` subcommand: scaffold, edit selections, drive
//! vendoring, and report status.
//!
//! The loader never reads the manifest or lock; this module is the only
//! writer of both.

use crate::definitions::lock::{self, Lock};
use crate::definitions::manifest::{self, Manifest, ManifestError};
use crate::definitions::pipeline::{self, PipelineError, Vendored};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum DefinitionsError {
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error(transparent)]
    Lock(#[from] lock::LockError),
    #[error(transparent)]
    Pipeline(#[from] PipelineError),
    #[error("{path}: definitions manifest already exists")]
    AlreadyExists { path: PathBuf },
    #[error("unknown source `{name}`; declare it in {}", manifest::MANIFEST_FILE)]
    UnknownSource { name: String },
    #[error("unknown definition kind `{kind}`; expected personas, actors, or prompts")]
    UnknownKind { kind: String },
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("definition drift:\n{details}")]
    Drift { details: String },
}

pub fn init(project_dir: &Path) -> Result<String, DefinitionsError> {
    let path = manifest::manifest_path(project_dir);
    if path.exists() {
        return Err(DefinitionsError::AlreadyExists { path });
    }
    std::fs::create_dir_all(project_dir).map_err(|source| DefinitionsError::Io {
        path: project_dir.into(),
        source,
    })?;
    std::fs::write(
        &path,
        r#"# Definition sources and selections for this repository.
#
# [[source]]
# name = "team"
# kind = "git"
#
# [source.git]
# url = "https://example.com/definitions.git"
# ref = "v1"
#
# [definitions.team]
# personas = ["reviewer"]
"#,
    )
    .map_err(|source| DefinitionsError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(format!("wrote {}", path.display()))
}

pub fn add(
    project_dir: &Path,
    source: &str,
    kind: &str,
    name: &str,
) -> Result<String, DefinitionsError> {
    validate_kind(kind)?;
    let mut manifest = Manifest::load(project_dir)?;
    manifest.validate(project_dir)?;
    if manifest.source(source).is_none() {
        return Err(DefinitionsError::UnknownSource {
            name: source.into(),
        });
    }
    let selected = manifest.definitions.entry(source.into()).or_default();
    let names = match kind {
        "personas" => &mut selected.personas,
        "actors" => &mut selected.actors,
        "prompts" => &mut selected.prompts,
        _ => unreachable!("validated"),
    };
    if names.iter().any(|existing| existing == name) {
        return Ok(format!("{kind}/{name} is already selected from `{source}`"));
    }
    names.push(name.into());
    store(project_dir, &manifest)?;
    Ok(format!("added {kind}/{name} from `{source}`"))
}

pub fn remove(
    project_dir: &Path,
    source: &str,
    kind: &str,
    name: &str,
) -> Result<String, DefinitionsError> {
    validate_kind(kind)?;
    let mut manifest = Manifest::load(project_dir)?;
    manifest.validate(project_dir)?;
    let Some(selected) = manifest.definitions.get_mut(source) else {
        return Err(DefinitionsError::UnknownSource {
            name: source.into(),
        });
    };
    let names = match kind {
        "personas" => &mut selected.personas,
        "actors" => &mut selected.actors,
        "prompts" => &mut selected.prompts,
        _ => unreachable!("validated"),
    };
    let before = names.len();
    names.retain(|existing| existing != name);
    if names.len() == before {
        return Ok(format!("{kind}/{name} is not selected from `{source}`"));
    }
    if selected.is_empty() {
        manifest.definitions.remove(source);
    }
    store(project_dir, &manifest)?;
    Ok(format!("removed {kind}/{name} from `{source}`"))
}

fn validate_kind(kind: &str) -> Result<(), DefinitionsError> {
    match kind {
        "personas" | "actors" | "prompts" => Ok(()),
        _ => Err(DefinitionsError::UnknownKind { kind: kind.into() }),
    }
}

fn store(project_dir: &Path, manifest: &Manifest) -> Result<(), DefinitionsError> {
    let path = manifest::manifest_path(project_dir);
    let body = toml::to_string_pretty(manifest).map_err(|err| ManifestError::Invalid {
        path: path.clone(),
        message: err.to_string(),
    })?;
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, body).map_err(|source| DefinitionsError::Io {
        path: temporary.clone(),
        source,
    })?;
    std::fs::rename(&temporary, &path).map_err(|source| DefinitionsError::Io {
        path: path.clone(),
        source,
    })
}

pub fn get(project_dir: &Path) -> Result<String, DefinitionsError> {
    Ok(format_vendored(pipeline::get(project_dir)?))
}

pub fn update(project_dir: &Path) -> Result<String, DefinitionsError> {
    Ok(format_vendored(pipeline::update(project_dir)?))
}

fn format_vendored(vendored: Vec<Vendored>) -> String {
    let lines = vendored
        .into_iter()
        .map(|definition| {
            format!(
                "{}/{} {} {}",
                definition.kind, definition.name, definition.source, definition.hash
            )
        })
        .collect::<Vec<_>>();
    if lines.is_empty() {
        "no definitions selected".into()
    } else {
        lines.join("\n")
    }
}

/// Re-hashes every vendored package against the lock. Any difference is
/// drift and is reported with a non-zero exit.
pub fn verify(project_dir: &Path) -> Result<String, DefinitionsError> {
    let lock = Lock::load(project_dir)?;
    let mut drifted = Vec::new();
    for (key, definition) in &lock.definitions {
        let Some((kind, name)) = key.split_once('/') else {
            drifted.push(format!("{key} malformed lock entry"));
            continue;
        };
        let path = project_dir.join(kind).join(name);
        if !path.is_dir() {
            drifted.push(format!("{key} missing"));
            continue;
        }
        let actual = lock::hash_package(&path)?;
        if actual != definition.hash {
            drifted.push(format!("{key} changed"));
        }
    }
    if drifted.is_empty() {
        Ok(format!("verified {} definitions", lock.definitions.len()))
    } else {
        drifted.sort();
        drifted.push("run `tackle definitions get` to restore the locked revisions".into());
        Err(DefinitionsError::Drift {
            details: drifted.join("\n"),
        })
    }
}

/// Reports every declared and locked definition with its status:
/// `ok`, `missing`, `drift`, `unlocked`, or `undeclared`.
pub fn list(project_dir: &Path) -> Result<String, DefinitionsError> {
    let manifest = Manifest::load(project_dir)?;
    manifest.validate(project_dir)?;
    let lock = match Lock::load(project_dir) {
        Ok(lock) => Some(lock),
        Err(lock::LockError::Missing { .. }) => None,
        Err(err) => return Err(err.into()),
    };

    let mut rows = Vec::new();
    for (source, selected) in &manifest.definitions {
        for (kind, name) in selected.entries() {
            let key = format!("{kind}/{name}");
            let path = project_dir.join(kind).join(name);
            let status = match (&lock, path.is_dir()) {
                (Some(lock), true) => match lock.definitions.get(&key) {
                    Some(locked) => {
                        if lock::hash_package(&path)? == locked.hash {
                            "ok"
                        } else {
                            "drift"
                        }
                    }
                    None => "unlocked",
                },
                (_, false) => "missing",
                (None, true) => "unlocked",
            };
            rows.push(format!("{key} {source} {status}"));
        }
    }
    if let Some(lock) = &lock {
        for (key, definition) in &lock.definitions {
            if !rows.iter().any(|row| row.starts_with(&format!("{key} "))) {
                rows.push(format!("{key} {} undeclared", definition.source));
            }
        }
    }
    rows.sort();
    Ok(rows.join("\n"))
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

    fn manifest(project: &Path) {
        std::fs::write(
            manifest::manifest_path(project),
            r#"
[[source]]
name = "local"
kind = "path"

[source.path]
dir = "shared"
"#,
        )
        .unwrap();
    }

    #[test]
    fn init_writes_a_parseable_manifest() {
        let (_dir, project) = project();
        init(&project).unwrap();
        Manifest::load(&project).unwrap();
        assert!(init(&project).is_err());
    }

    #[test]
    fn add_and_remove_update_selections() {
        let (_dir, project) = project();
        manifest(&project);
        add(&project, "local", "personas", "reviewer").unwrap();
        let loaded = Manifest::load(&project).unwrap();
        assert_eq!(
            loaded.definitions["local"].personas,
            vec!["reviewer".to_string()]
        );
        remove(&project, "local", "personas", "reviewer").unwrap();
        let loaded = Manifest::load(&project).unwrap();
        assert!(!loaded.definitions.contains_key("local"));
    }

    #[test]
    fn verify_reports_drift_and_accepts_a_clean_tree() {
        let (dir, project) = project();
        let shared = dir.path().join("shared");
        std::fs::create_dir_all(shared.join("personas").join("reviewer")).unwrap();
        std::fs::write(
            shared.join("personas/reviewer/PERSONA.md"),
            "---\ndescription: Review\n---\nBody\n",
        )
        .unwrap();
        std::fs::write(shared.join("personas/reviewer/notes.txt"), "notes\n").unwrap();
        manifest(&project);
        add(&project, "local", "personas", "reviewer").unwrap();
        update(&project).unwrap();
        assert!(verify(&project).unwrap().contains("verified"));

        std::fs::write(project.join("personas/reviewer/notes.txt"), "edited\n").unwrap();
        let err = verify(&project).unwrap_err();
        assert!(err.to_string().contains("personas/reviewer"), "{err}");
    }

    #[test]
    fn verify_without_a_lock_names_get() {
        let (_dir, project) = project();
        std::fs::write(manifest::manifest_path(&project), "# empty\n").unwrap();
        let err = verify(&project).unwrap_err();
        assert!(err.to_string().contains("definitions get"), "{err}");
    }

    #[test]
    fn list_reports_declared_and_vendored_definitions() {
        let (dir, project) = project();
        let shared = dir.path().join("shared");
        std::fs::create_dir_all(shared.join("personas").join("reviewer")).unwrap();
        std::fs::write(
            shared.join("personas/reviewer/PERSONA.md"),
            "---\ndescription: Review\n---\nBody\n",
        )
        .unwrap();
        manifest(&project);
        add(&project, "local", "personas", "reviewer").unwrap();
        update(&project).unwrap();

        let listed = list(&project).unwrap();
        assert!(listed.contains("personas/reviewer"), "{listed}");
        assert!(listed.contains("ok"), "{listed}");
    }
}
