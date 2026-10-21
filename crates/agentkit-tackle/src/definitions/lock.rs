//! The definitions lock: the immutable revisions and integrity hashes of
//! everything vendored into the repository tree.
//!
//! The lock carries integrity only. Loading definitions never consults it;
//! drift is reported by `definitions verify`.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const LOCK_FILE: &str = "definitions.lock";
pub const LOCK_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("{path}: definitions lock is missing; run `tackle definitions get`")]
    Missing { path: PathBuf },
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: invalid definitions lock: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("{path}: unsupported definitions lock version {version}")]
    Version { path: PathBuf, version: u32 },
    #[error("{path}: definition package contains non-UTF-8 path")]
    NonUtf8Path { path: PathBuf },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lock {
    pub version: u32,
    #[serde(default)]
    pub sources: BTreeMap<String, LockedSource>,
    #[serde(default)]
    pub definitions: BTreeMap<String, LockedDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedSource {
    /// The immutable commit the declared ref resolved to.
    pub commit: String,
    /// The url as declared, for diagnostics.
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedDefinition {
    /// The source that provided the definition.
    pub source: String,
    /// The content hash: sorted files, each as relative path, length and
    /// bytes. Flat definitions hash as their bare file content.
    pub hash: String,
}

impl Lock {
    pub fn new() -> Self {
        Self {
            version: LOCK_VERSION,
            ..Self::default()
        }
    }

    pub fn load(project_dir: &Path) -> Result<Self, LockError> {
        let path = lock_path(project_dir);
        let source = std::fs::read_to_string(&path).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                LockError::Missing { path: path.clone() }
            } else {
                LockError::Io {
                    path: path.clone(),
                    source,
                }
            }
        })?;
        Self::parse(&source, &path)
    }

    pub fn parse(source: &str, path: &Path) -> Result<Self, LockError> {
        let lock: Lock = toml::from_str(source).map_err(|err| LockError::Parse {
            path: path.into(),
            message: err.to_string(),
        })?;
        if lock.version != LOCK_VERSION {
            return Err(LockError::Version {
                path: path.into(),
                version: lock.version,
            });
        }
        Ok(lock)
    }

    /// Writes atomically: a failed write leaves any existing lock intact.
    pub fn store(&self, project_dir: &Path) -> Result<(), LockError> {
        let path = lock_path(project_dir);
        let body = toml::to_string_pretty(self).map_err(|err| LockError::Parse {
            path: path.clone(),
            message: err.to_string(),
        })?;
        let temporary = path.with_extension("lock.tmp");
        std::fs::write(&temporary, body).map_err(|source| LockError::Io {
            path: temporary.clone(),
            source,
        })?;
        std::fs::rename(&temporary, &path).map_err(|source| LockError::Io {
            path: path.clone(),
            source,
        })
    }
}

pub fn lock_path(project_dir: &Path) -> PathBuf {
    project_dir.join(LOCK_FILE)
}

/// Hashes a flat definition as its bare file content.
pub fn hash_file(path: &Path) -> Result<String, LockError> {
    let bytes = std::fs::read(path).map_err(|source| LockError::Io {
        path: path.into(),
        source,
    })?;
    Ok(sha256(&bytes))
}

/// Hashes a package over its sorted files: relative path, length and
/// bytes, in that order. Creation order and filesystem enumeration order
/// do not affect the result.
pub fn hash_package(directory: &Path) -> Result<String, LockError> {
    let mut files = Vec::new();
    walk(directory, directory, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));

    let mut hasher = Sha256::new();
    for (relative, full) in files {
        let bytes = std::fs::read(&full).map_err(|source| LockError::Io {
            path: full.clone(),
            source,
        })?;
        hasher.update(relative.as_bytes());
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(&bytes);
    }
    Ok(hex(hasher.finalize()))
}

/// The entry point is part of the package: it is the definition.
fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), LockError> {
    let entries = std::fs::read_dir(dir).map_err(|source| LockError::Io {
        path: dir.into(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| LockError::Io {
            path: dir.into(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.metadata().map_err(|source| LockError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            walk(root, &path, out)?;
        } else if file_type.is_file() {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            let relative = relative
                .to_str()
                .ok_or_else(|| LockError::NonUtf8Path { path: path.clone() })?
                .replace('\\', "/");
            out.push((relative, path));
        }
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    hex(Sha256::digest(bytes))
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".agentkit").join("tackle")).unwrap();
        dir
    }

    #[test]
    fn a_lock_round_trips() {
        let dir = project();
        let project = dir.path().join(".agentkit").join("tackle");
        let mut lock = Lock::new();
        lock.sources.insert(
            "team".into(),
            LockedSource {
                commit: "0123456789abcdef".into(),
                url: "https://example.com/team.git".into(),
                revision: Some("v1".into()),
            },
        );
        lock.definitions.insert(
            "prompts/triage".into(),
            LockedDefinition {
                source: "team".into(),
                hash: "deadbeef".into(),
            },
        );
        lock.store(&project).unwrap();
        assert_eq!(Lock::load(&project).unwrap(), lock);
    }

    #[test]
    fn a_missing_lock_names_get() {
        let dir = project();
        let project = dir.path().join(".agentkit").join("tackle");
        let err = Lock::load(&project).unwrap_err();
        assert!(err.to_string().contains("definitions get"), "{err}");
    }

    #[test]
    fn package_hashing_is_stable_across_file_order() {
        let first = tempfile::tempdir().unwrap();
        std::fs::write(first.path().join("b.md"), "b").unwrap();
        std::fs::create_dir(first.path().join("nested")).unwrap();
        std::fs::write(first.path().join("a.md"), "a").unwrap();
        std::fs::write(first.path().join("nested").join("c.md"), "c").unwrap();

        let second = tempfile::tempdir().unwrap();
        std::fs::create_dir(second.path().join("nested")).unwrap();
        std::fs::write(second.path().join("nested").join("c.md"), "c").unwrap();
        std::fs::write(second.path().join("a.md"), "a").unwrap();
        std::fs::write(second.path().join("b.md"), "b").unwrap();

        assert_eq!(
            hash_package(first.path()).unwrap(),
            hash_package(second.path()).unwrap()
        );
    }

    #[test]
    fn package_hashing_changes_when_any_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("entry.md"), "entry").unwrap();
        std::fs::write(dir.path().join("resource.txt"), "before").unwrap();
        let before = hash_package(dir.path()).unwrap();
        std::fs::write(dir.path().join("resource.txt"), "after").unwrap();
        assert_ne!(before, hash_package(dir.path()).unwrap());
    }

    #[test]
    fn package_hashing_separates_paths_from_content() {
        let first = tempfile::tempdir().unwrap();
        std::fs::write(first.path().join("a"), "bc").unwrap();
        let second = tempfile::tempdir().unwrap();
        std::fs::write(second.path().join("ab"), "c").unwrap();
        assert_ne!(
            hash_package(first.path()).unwrap(),
            hash_package(second.path()).unwrap()
        );
    }

    #[test]
    fn a_flat_definition_hashes_as_its_file_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flat.md");
        std::fs::write(&path, "---\nname: flat\n---\nbody\n").unwrap();
        assert_eq!(
            hash_file(&path).unwrap(),
            sha256(b"---\nname: flat\n---\nbody\n")
        );
    }
}
