use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use crate::domain::{SandboxError, SandboxMetadata};

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

fn store_in(dir: &Path, slug: &str, meta: &SandboxMetadata) -> Result<(), SandboxError> {
    fs::create_dir_all(dir).map_err(SandboxError::Io)?;

    let path = dir.join(format!("{}.toml", slug));
    let content = toml::to_string(meta).map_err(|e| SandboxError::Config(e.to_string()))?;

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

fn load_in(dir: &Path, slug: &str) -> Result<Option<SandboxMetadata>, SandboxError> {
    let path = dir.join(format!("{}.toml", slug));
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path).map_err(SandboxError::Io)?;
    let meta: SandboxMetadata =
        toml::from_str(&content).map_err(|e| SandboxError::Config(e.to_string()))?;
    Ok(Some(meta))
}

fn remove_in(dir: &Path, slug: &str) -> Result<(), SandboxError> {
    let path = dir.join(format!("{}.toml", slug));
    if path.exists() {
        fs::remove_file(&path).map_err(SandboxError::Io)?;
    }
    Ok(())
}

pub fn store(project_slug: &str, slug: &str, meta: &SandboxMetadata) -> Result<(), SandboxError> {
    let dir = base_path(project_slug, load_metadata_dir().as_deref());
    store_in(&dir, slug, meta)
}

pub fn load(project_slug: &str, slug: &str) -> Result<Option<SandboxMetadata>, SandboxError> {
    let dir = base_path(project_slug, load_metadata_dir().as_deref());
    load_in(&dir, slug)
}

pub fn remove(project_slug: &str, slug: &str) -> Result<(), SandboxError> {
    let dir = base_path(project_slug, load_metadata_dir().as_deref());
    remove_in(&dir, slug)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use crate::domain::{SandboxMetadata, SandboxStatus, ScmMode};

    use super::*;

    fn test_metadata() -> SandboxMetadata {
        SandboxMetadata {
            name: "test-sandbox".to_string(),
            branch_name: "litterbox/test-sandbox".to_string(),
            container_id: "litterbox-test-abc123".to_string(),
            status: SandboxStatus::Active,
            mode: ScmMode::Remote,
            project_slug: "my-project".to_string(),
            forwarded_ports: Vec::new(),
        }
    }

    #[test]
    fn store_and_load_roundtrip() {
        let dir = TempDir::new().expect("tempdir");
        let meta = test_metadata();
        store_in(dir.path(), "test-sandbox", &meta).expect("store");
        let loaded = load_in(dir.path(), "test-sandbox")
            .expect("load")
            .expect("some");
        assert_eq!(loaded.name, meta.name);
        assert_eq!(loaded.branch_name, meta.branch_name);
        assert_eq!(loaded.container_id, meta.container_id);
        assert_eq!(loaded.status, meta.status);
        assert_eq!(loaded.mode, meta.mode);
        assert_eq!(loaded.project_slug, meta.project_slug);
        assert_eq!(loaded.forwarded_ports, meta.forwarded_ports);
    }

    #[test]
    fn store_and_load_direct_mode() {
        let dir = TempDir::new().expect("tempdir");
        let meta = SandboxMetadata {
            name: "legacy-sandbox".to_string(),
            branch_name: "litterbox/legacy-sandbox".to_string(),
            container_id: "litterbox-test-legacy".to_string(),
            status: SandboxStatus::Active,
            mode: ScmMode::Direct,
            project_slug: "my-project".to_string(),
            forwarded_ports: Vec::new(),
        };
        store_in(dir.path(), "legacy-sandbox", &meta).expect("store");
        let loaded = load_in(dir.path(), "legacy-sandbox")
            .expect("load")
            .expect("some");
        assert_eq!(loaded.mode, ScmMode::Direct);
        assert_eq!(loaded.project_slug, "my-project");
    }

    #[test]
    fn load_missing_returns_none() {
        let dir = TempDir::new().expect("tempdir");
        let result = load_in(dir.path(), "non-existent").expect("load");
        assert!(result.is_none());
    }

    #[test]
    fn remove_deletes_file() {
        let dir = TempDir::new().expect("tempdir");
        let meta = test_metadata();
        store_in(dir.path(), "test-sandbox", &meta).expect("store");
        assert!(load_in(dir.path(), "test-sandbox").expect("load").is_some());

        remove_in(dir.path(), "test-sandbox").expect("remove");
        assert!(load_in(dir.path(), "test-sandbox").expect("load").is_none());
    }

    #[test]
    fn remove_noop_when_missing() {
        let dir = TempDir::new().expect("tempdir");
        remove_in(dir.path(), "non-existent").expect("remove");
    }

    #[test]
    fn base_path_uses_data_dir_by_default() {
        let path = base_path("my-project", None);
        let lossy = path.to_string_lossy();
        assert!(lossy.contains("metadata/my-project"), "path: {lossy}");
    }

    #[test]
    fn public_api_store_and_load_end_to_end() {
        let tmp = TempDir::new().expect("tempdir");
        let dir = base_path("my-project", Some(tmp.path()));

        let meta = test_metadata();
        store_in(&dir, "my-sandbox", &meta).expect("store");
        let loaded = load_in(&dir, "my-sandbox").expect("load").expect("some");
        assert_eq!(loaded.name, meta.name);
        assert_eq!(loaded.mode, meta.mode);

        remove_in(&dir, "my-sandbox").expect("remove");
        assert!(load_in(&dir, "my-sandbox").expect("load").is_none());
    }
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
