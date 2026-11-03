use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use crate::domain::SandboxError;

const RETRY_MAX: u32 = 3;
const RETRY_BASE_MS: u64 = 50;

fn base_path(project_slug: &str, metadata_dir: Option<&Path>) -> PathBuf {
    match metadata_dir {
        Some(dir) => dir.join(project_slug),
        None => crate::vcs_store::path::data_dir()
            .join("metadata")
            .join(project_slug),
    }
}

fn load_metadata_dir() -> Option<PathBuf> {
    std::env::var("LITTERBOX_METADATA_DIR")
        .ok()
        .map(PathBuf::from)
}

fn remove_in(dir: &Path, slug: &str) -> Result<(), SandboxError> {
    let path = dir.join(format!("{}.toml", slug));
    if path.exists() {
        fs::remove_file(&path).map_err(SandboxError::Io)?;
    }
    Ok(())
}

/// A sandbox as persisted: the contract's record plus the branch it was paired
/// with. The SCM mode stays out of the contract on purpose.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct StoredSandbox {
    /// The contract record, which names the backend and carries its payload.
    pub sandbox: crate::sandbox::contract::SandboxRecord,
    /// The branch the orchestrator created for this sandbox.
    pub branch_name: String,
    /// Whether the branch lives in a bare clone rather than the working repo.
    pub remote: bool,
}

/// What a metadata file turned out to be.
#[derive(Clone, Debug)]
pub enum LoadedSandbox {
    /// A record written by this version.
    Current(StoredSandbox),
    /// A record written by an earlier release. It is reported, never adopted.
    Legacy {
        /// The file holding it, so the user can remove it.
        path: PathBuf,
    },
}

impl LoadedSandbox {
    /// The contract record, when this is a current record.
    pub fn record(&self) -> Option<&crate::sandbox::contract::SandboxRecord> {
        match self {
            Self::Current(stored) => Some(&stored.sandbox),
            Self::Legacy { .. } => None,
        }
    }
}

/// The directory holding one project's records.
pub fn directory(project_slug: &str) -> PathBuf {
    base_path(project_slug, load_metadata_dir().as_deref())
}

/// Persist a sandbox record.
pub fn store_sandbox(project_slug: &str, stored: &StoredSandbox) -> Result<(), SandboxError> {
    let dir = base_path(project_slug, load_metadata_dir().as_deref());
    store_value_in(&dir, &stored.sandbox.name, stored)
}

/// Load a sandbox record, distinguishing a current record from a legacy one.
pub fn load_sandbox(project_slug: &str, name: &str) -> Result<Option<LoadedSandbox>, SandboxError> {
    let dir = base_path(project_slug, load_metadata_dir().as_deref());
    load_sandbox_in(&dir, name)
}

/// Remove a sandbox record.
pub fn remove_sandbox(project_slug: &str, name: &str) -> Result<(), SandboxError> {
    let dir = base_path(project_slug, load_metadata_dir().as_deref());
    remove_in(&dir, name)
}

fn store_value_in<T: serde::Serialize>(
    dir: &Path,
    slug: &str,
    value: &T,
) -> Result<(), SandboxError> {
    fs::create_dir_all(dir).map_err(SandboxError::Io)?;

    let path = dir.join(format!("{}.toml", slug));
    let content = toml::to_string(value).map_err(|e| SandboxError::Config(e.to_string()))?;

    let file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .map_err(SandboxError::Io)?;

    let mut attempt = 0u32;
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => {
                attempt += 1;
                if attempt > RETRY_MAX {
                    return Err(SandboxError::Io(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        "metadata lock held by another process",
                    )));
                }
                sleep(Duration::from_millis(RETRY_BASE_MS * attempt as u64));
            }
            Err(e) => return Err(SandboxError::Io(e.into())),
        }
    }

    file.set_len(0).map_err(SandboxError::Io)?;
    (&file)
        .write_all(content.as_bytes())
        .map_err(SandboxError::Io)?;
    file.sync_all().map_err(SandboxError::Io)?;

    Ok(())
}

fn load_sandbox_in(dir: &Path, name: &str) -> Result<Option<LoadedSandbox>, SandboxError> {
    let path = dir.join(format!("{}.toml", name));
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path).map_err(SandboxError::Io)?;
    let value: toml::Value =
        toml::from_str(&content).map_err(|e| SandboxError::Config(e.to_string()))?;

    // A file with a nested sandbox record is ours. A file with a container id
    // and no such record was written before the record changed, and is
    // reported, not adopted.
    if value.get("sandbox").is_none() {
        return Ok(Some(LoadedSandbox::Legacy { path }));
    }

    let stored: StoredSandbox = value
        .try_into()
        .map_err(|e: toml::de::Error| SandboxError::Config(e.to_string()))?;
    Ok(Some(LoadedSandbox::Current(stored)))
}

#[cfg(test)]
mod record_tests {
    use std::path::Path;

    use tempfile::TempDir;

    use super::{LoadedSandbox, StoredSandbox, load_sandbox_in, remove_in, store_value_in};
    use crate::sandbox::contract::SandboxRecord;

    fn stored() -> StoredSandbox {
        StoredSandbox {
            sandbox: SandboxRecord {
                name: "demo".to_string(),
                backend: "docker".to_string(),
                project_slug: "project".to_string(),
                data: toml::Value::Table(Default::default()),
            },
            branch_name: "litterbox/demo".to_string(),
            remote: false,
        }
    }

    #[test]
    fn a_record_round_trips_with_its_backend_and_branch() {
        let dir = TempDir::new().expect("tempdir");
        store_value_in(dir.path(), "demo", &stored()).expect("store");

        let loaded = load_sandbox_in(dir.path(), "demo")
            .expect("load")
            .expect("present");
        match loaded {
            LoadedSandbox::Current(current) => {
                assert_eq!(current.sandbox.backend, "docker");
                assert_eq!(current.branch_name, "litterbox/demo");
                assert!(!current.remote);
            }
            other => panic!("expected a current record, got {other:?}"),
        }
    }

    #[test]
    fn a_legacy_record_is_reported_and_never_adopted() {
        let dir = TempDir::new().expect("tempdir");
        let legacy = "name = 'demo'\nbranch_name = 'litterbox/demo'\ncontainer_id = 'abc123'\nstatus = 'Active'\nmode = 'Direct'\nproject_slug = 'project'\nforwarded_ports = []\n";
        std::fs::write(dir.path().join("demo.toml"), legacy).expect("write");

        let loaded = load_sandbox_in(dir.path(), "demo")
            .expect("load")
            .expect("present");
        match &loaded {
            LoadedSandbox::Legacy { path } => {
                assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("demo.toml"));
            }
            other => panic!("expected a legacy record, got {other:?}"),
        }
        assert!(loaded.record().is_none());
    }

    #[test]
    fn a_missing_record_is_none() {
        let dir = TempDir::new().expect("tempdir");
        assert!(
            load_sandbox_in(dir.path(), "absent")
                .expect("load")
                .is_none()
        );
    }

    #[test]
    fn removing_a_record_removes_the_file() {
        let dir = TempDir::new().expect("tempdir");
        store_value_in(dir.path(), "demo", &stored()).expect("store");
        remove_in(dir.path(), "demo").expect("remove");
        assert!(load_sandbox_in(dir.path(), "demo").expect("load").is_none());
        assert!(!Path::new(&dir.path().join("demo.toml")).exists());
    }
}
