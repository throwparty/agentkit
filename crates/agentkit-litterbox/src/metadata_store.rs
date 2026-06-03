use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use crate::domain::{SandboxError, SandboxMetadata};

const RETRY_MAX: u32 = 3;
const RETRY_BASE_MS: u64 = 50;

fn base_path(project_slug: &str) -> PathBuf {
    if let Ok(override_dir) = std::env::var("LITTERBOX_METADATA_DIR") {
        return PathBuf::from(override_dir).join(project_slug);
    }
    crate::vcs_store::path::data_dir()
        .join("metadata")
        .join(project_slug)
}

fn store_in(dir: &Path, slug: &str, meta: &SandboxMetadata) -> Result<(), SandboxError> {
    fs::create_dir_all(dir).map_err(SandboxError::Io)?;

    let path = dir.join(format!("{}.toml", slug));
    let content =
        toml::to_string(meta).map_err(|e| SandboxError::Config(e.to_string()))?;

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

pub fn store(
    project_slug: &str,
    slug: &str,
    meta: &SandboxMetadata,
) -> Result<(), SandboxError> {
    store_in(&base_path(project_slug), slug, meta)
}

pub fn load(
    project_slug: &str,
    slug: &str,
) -> Result<Option<SandboxMetadata>, SandboxError> {
    load_in(&base_path(project_slug), slug)
}

pub fn remove(project_slug: &str, slug: &str) -> Result<(), SandboxError> {
    remove_in(&base_path(project_slug), slug)
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
        assert!(load_in(dir.path(), "test-sandbox")
            .expect("load")
            .is_some());

        remove_in(dir.path(), "test-sandbox").expect("remove");
        assert!(load_in(dir.path(), "test-sandbox")
            .expect("load")
            .is_none());
    }

    #[test]
    fn remove_noop_when_missing() {
        let dir = TempDir::new().expect("tempdir");
        remove_in(dir.path(), "non-existent").expect("remove");
    }

    #[test]
    fn base_path_uses_data_dir_by_default() {
        let path = base_path("my-project");
        let lossy = path.to_string_lossy();
        assert!(lossy.contains("metadata/my-project"), "path: {lossy}");
    }

    #[test]
    fn public_api_store_and_load_end_to_end() {
        // Exercises the full public API: path resolution (via
        // LITTERBOX_METADATA_DIR override) + file I/O together.
        let tmp = TempDir::new().expect("tempdir");
        unsafe {
            std::env::set_var("LITTERBOX_METADATA_DIR", tmp.path());
        }

        let meta = test_metadata();
        store("my-project", "my-sandbox", &meta).expect("store");
        let loaded = load("my-project", "my-sandbox")
            .expect("load")
            .expect("some");
        assert_eq!(loaded.name, meta.name);
        assert_eq!(loaded.mode, meta.mode);

        remove("my-project", "my-sandbox").expect("remove");
        assert!(load("my-project", "my-sandbox")
            .expect("load")
            .is_none());

        unsafe {
            std::env::remove_var("LITTERBOX_METADATA_DIR");
        }
    }
}
