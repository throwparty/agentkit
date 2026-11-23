use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::sandbox::contract::Error;

fn remove_in(dir: &Path, slug: &str) -> Result<(), Error> {
    let path = dir.join(format!("{}.toml", slug));
    if path.exists() {
        fs::remove_file(&path).map_err(io_error)?;
    }
    Ok(())
}

/// A held reconciliation: the three revisions a host merge needs.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct StoredMerge {
    /// The revision both sides last agreed on.
    pub base: String,
    /// The sandbox branch tip.
    pub ours: String,
    /// The branch tip.
    pub theirs: String,
}

/// A resolution the agent is working on in the sandbox.
///
/// Only the revision both sides agreed on is kept. The revision under
/// resolution is read from the working copy, because resolving rewrites it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct StoredResolution {
    /// The revision both sides last agreed on when resolution began.
    pub synced: String,
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
    /// How the workspace was presented. A record written before the choice
    /// existed has no field, which is a legacy fact rather than a default.
    #[serde(default)]
    pub binding: Option<crate::sandbox::contract::WorkspaceBinding>,
    /// The VCS tool that owns this sandbox's source. A record written before
    /// the choice existed has no field, which is a legacy fact, not a default.
    #[serde(default)]
    pub vcs: Option<String>,
    /// The held reconciliation, when one is pending.
    #[serde(default)]
    pub merge: Option<StoredMerge>,
    /// The resolution the agent is working on, when one is open.
    #[serde(default)]
    pub resolution: Option<StoredResolution>,
}

impl StoredSandbox {
    /// The binding this sandbox was created with.
    ///
    /// A record without the field predates the choice, and it was created as a
    /// private copy, so it reads as `Copied` without being rewritten.
    pub fn binding(&self) -> crate::sandbox::contract::WorkspaceBinding {
        self.binding
            .unwrap_or(crate::sandbox::contract::WorkspaceBinding::Copied)
    }

    /// The tool this sandbox's source belongs to.
    ///
    /// A record without the field predates the choice and was a Git sandbox.
    pub fn vcs(&self) -> &str {
        self.vcs.as_deref().unwrap_or("git")
    }
}

/// What a metadata file turned out to be.
#[derive(Clone, Debug)]
pub enum LoadedSandbox {
    /// A record written by this version.
    Current(Box<StoredSandbox>),
    /// A record that cannot be read at all.
    Corrupt {
        /// The file holding it.
        path: PathBuf,
        /// What went wrong.
        reason: String,
    },
}

impl LoadedSandbox {
    /// The contract record, when this is a current record.
    pub fn record(&self) -> Option<&crate::sandbox::contract::SandboxRecord> {
        match self {
            Self::Current(stored) => Some(&stored.sandbox),
            Self::Corrupt { .. } => None,
        }
    }
}

/// The directory holding one project's records.
pub fn directory(project_slug: &str) -> PathBuf {
    agentkit_common::data_dir("litterbox")
        .join("metadata")
        .join(project_slug)
}

/// Persist a record into an explicit directory.
pub fn store_sandbox_at(dir: &Path, name: &str, stored: &StoredSandbox) -> Result<(), Error> {
    store_value_in(dir, name, stored)
}

/// Load a record from an explicit directory.
pub fn load_sandbox_at(dir: &Path, name: &str) -> Result<Option<LoadedSandbox>, Error> {
    load_sandbox_in(dir, name)
}

/// One record file in a directory, whether it loaded or not.
#[derive(Debug)]
pub struct RecordEntry {
    /// The record's name, from its file name.
    pub name: String,
    /// What the file turned out to be.
    pub loaded: Result<LoadedSandbox, Error>,
}

/// Every record file in a directory, including ones that cannot be read.
pub fn entries_in(dir: &Path) -> Vec<RecordEntry> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let name = name.strip_suffix(".toml")?.to_string();
            match load_sandbox_in(dir, &name) {
                Ok(Some(loaded)) => Some(RecordEntry {
                    name,
                    loaded: Ok(loaded),
                }),
                Ok(None) => None,
                Err(error) => Some(RecordEntry {
                    name,
                    loaded: Err(error),
                }),
            }
        })
        .collect()
}

/// Remove a record from an explicit directory.
pub fn remove_sandbox_at(dir: &Path, name: &str) -> Result<(), Error> {
    remove_in(dir, name)
}

fn store_value_in<T: serde::Serialize>(dir: &Path, slug: &str, value: &T) -> Result<(), Error> {
    fs::create_dir_all(dir).map_err(io_error)?;
    #[cfg(unix)]
    {
        // Records name containers, ports and hosts; keep them, and the shared
        // metadata root this module creates, to the user.
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
        if let Some(parent) = dir.parent().filter(|parent| {
            parent
                .file_name()
                .is_some_and(|name| name == std::ffi::OsStr::new("metadata"))
        }) {
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
        }
    }

    let path = dir.join(format!("{}.toml", slug));
    let content = toml::to_string(value).map_err(|error| Error::failed(error.to_string()))?;

    // Write beside the target and rename, so a reader observes either the old
    // record or the new one and never a truncated file.
    let staging = dir.join(format!(".{}.toml.new", slug));
    let staged = (|| -> std::io::Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&staging)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()
    })();
    if let Err(error) = staged {
        let _ = fs::remove_file(&staging);
        return Err(io_error(error));
    }
    fs::rename(&staging, &path).map_err(io_error)?;

    // The rename is only durable once the directory entry is synced.
    #[cfg(unix)]
    {
        fs::File::open(dir)
            .and_then(|handle| handle.sync_all())
            .map_err(io_error)?;
    }

    Ok(())
}

fn load_sandbox_in(dir: &Path, name: &str) -> Result<Option<LoadedSandbox>, Error> {
    let path = dir.join(format!("{}.toml", name));
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path).map_err(io_error)?;
    if content.trim().is_empty() {
        return Ok(Some(LoadedSandbox::Corrupt {
            path,
            reason: "the record is empty".to_string(),
        }));
    }

    let value: toml::Value = match toml::from_str(&content) {
        Ok(value) => value,
        Err(error) => {
            return Ok(Some(LoadedSandbox::Corrupt {
                path,
                reason: format!("the record is not valid TOML: {error}"),
            }));
        }
    };

    // A file with a nested sandbox record is ours. Anything else, including a
    // file written before the record changed, cannot be read.
    if value.get("sandbox").is_none() {
        return Ok(Some(LoadedSandbox::Corrupt {
            path,
            reason: "the record has no 'sandbox' table".to_string(),
        }));
    }

    match value.try_into::<StoredSandbox>() {
        Ok(stored) if stored.sandbox.name != name => Ok(Some(LoadedSandbox::Corrupt {
            path,
            reason: format!(
                "the record names '{}' but is filed as '{name}'",
                stored.sandbox.name
            ),
        })),
        Ok(stored) => Ok(Some(LoadedSandbox::Current(Box::new(stored)))),
        Err(error) => Ok(Some(LoadedSandbox::Corrupt {
            path,
            reason: format!("the record does not match the schema: {error}"),
        })),
    }
}

fn io_error(error: std::io::Error) -> Error {
    Error::failed(error.to_string())
}

#[cfg(test)]
mod record_tests {
    use std::path::Path;

    use tempfile::TempDir;

    use super::{
        LoadedSandbox, StoredMerge, StoredSandbox, load_sandbox_in, remove_in, store_value_in,
    };
    use crate::sandbox::contract::{SandboxRecord, WorkspaceBinding};

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
            binding: None,
            vcs: None,
            merge: None,
            resolution: None,
        }
    }

    #[test]
    fn a_record_whose_name_does_not_match_its_file_is_corrupt() {
        let dir = TempDir::new().expect("tempdir");
        store_value_in(dir.path(), "filed-as", &stored()).expect("store");

        match load_sandbox_in(dir.path(), "filed-as").expect("load") {
            Some(LoadedSandbox::Corrupt { reason, .. }) => {
                assert!(reason.contains("demo"), "{reason}");
                assert!(reason.contains("filed-as"), "{reason}");
            }
            other => panic!("expected a corrupt record, got {other:?}"),
        }
    }

    #[test]
    fn a_record_without_a_binding_reads_as_copied() {
        assert!(stored().binding.is_none(), "the fixture is a legacy record");
        assert_eq!(stored().binding(), WorkspaceBinding::Copied);
    }

    #[test]
    fn a_legacy_record_loads_and_is_never_rewritten() {
        let dir = TempDir::new().expect("tempdir");
        store_value_in(dir.path(), "demo", &stored()).expect("store");
        let path = dir.path().join("demo.toml");
        let before = std::fs::read_to_string(&path).expect("read");
        assert!(!before.contains("binding"), "{before}");

        let loaded = load_sandbox_in(dir.path(), "demo")
            .expect("load")
            .expect("present");
        let LoadedSandbox::Current(current) = loaded else {
            panic!("expected a current record");
        };

        assert_eq!(current.binding(), WorkspaceBinding::Copied);
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            before,
            "loading does not rewrite the record"
        );
    }

    #[test]
    fn a_recorded_binding_round_trips() {
        let dir = TempDir::new().expect("tempdir");
        let mut bound = stored();
        bound.binding = Some(WorkspaceBinding::Bound);
        store_value_in(dir.path(), "demo", &bound).expect("store");

        match load_sandbox_in(dir.path(), "demo")
            .expect("load")
            .expect("present")
        {
            LoadedSandbox::Current(current) => {
                assert_eq!(current.binding(), WorkspaceBinding::Bound);
            }
            other => panic!("expected a current record, got {other:?}"),
        }
    }

    #[test]
    fn a_record_without_a_merge_reads_as_none() {
        assert!(stored().merge.is_none(), "the fixture is a legacy record");
    }

    #[test]
    fn a_recorded_merge_round_trips() {
        let dir = TempDir::new().expect("tempdir");
        let mut held = stored();
        held.merge = Some(StoredMerge {
            base: "aaaa".to_string(),
            ours: "bbbb".to_string(),
            theirs: "cccc".to_string(),
        });
        store_value_in(dir.path(), "demo", &held).expect("store");

        match load_sandbox_in(dir.path(), "demo")
            .expect("load")
            .expect("present")
        {
            LoadedSandbox::Current(current) => {
                assert_eq!(current.merge, held.merge);
            }
            other => panic!("expected a current record, got {other:?}"),
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
    fn a_record_without_a_sandbox_table_is_corrupt() {
        let dir = TempDir::new().expect("tempdir");
        let old = "name = 'demo'\nbranch_name = 'litterbox/demo'\ncontainer_id = 'abc123'\nstatus = 'Active'\nmode = 'Direct'\nproject_slug = 'project'\nforwarded_ports = []\n";
        std::fs::write(dir.path().join("demo.toml"), old).expect("write");

        let loaded = load_sandbox_in(dir.path(), "demo")
            .expect("load")
            .expect("present");
        match &loaded {
            LoadedSandbox::Corrupt { path, reason } => {
                assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("demo.toml"));
                assert!(reason.contains("sandbox"), "{reason}");
            }
            other => panic!("expected a corrupt record, got {other:?}"),
        }
        assert!(loaded.record().is_none());
    }

    #[test]
    fn an_empty_record_is_corrupt() {
        let dir = TempDir::new().expect("tempdir");
        std::fs::write(dir.path().join("demo.toml"), "").expect("write");

        match load_sandbox_in(dir.path(), "demo").expect("load") {
            Some(LoadedSandbox::Corrupt { reason, .. }) => {
                assert!(reason.contains("empty"), "{reason}");
            }
            other => panic!("expected a corrupt record, got {other:?}"),
        }
    }

    #[test]
    fn unparsable_toml_is_corrupt() {
        let dir = TempDir::new().expect("tempdir");
        std::fs::write(dir.path().join("demo.toml"), "this is not toml").expect("write");

        match load_sandbox_in(dir.path(), "demo").expect("load") {
            Some(LoadedSandbox::Corrupt { .. }) => {}
            other => panic!("expected a corrupt record, got {other:?}"),
        }
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
