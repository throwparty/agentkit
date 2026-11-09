//! The payload a sandbox record carries.
//!
//! It holds what a later process needs to act on the sandbox and nothing else.
//! Values never appear: the environment travels through the sandbox itself and
//! the payload keeps only provenance.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::BACKEND;
use super::backend::Network;
use super::view::{Access, Mount};
use crate::sandbox::contract::{Error, SandboxRecord};
use crate::sandbox::environment::Provenance;

/// The payload version this release writes.
pub const VERSION: u32 = 2;

/// One mount as the record holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedMount {
    /// The host path, which is also the sandbox path.
    pub path: String,
    /// Whether the mount is read-only.
    pub read_only: bool,
}

impl From<&Mount> for RecordedMount {
    fn from(mount: &Mount) -> Self {
        Self {
            path: mount.path.to_string_lossy().to_string(),
            read_only: mount.access == Access::ReadOnly,
        }
    }
}

impl RecordedMount {
    /// The mount this record describes.
    pub fn mount(&self) -> Mount {
        Mount {
            path: PathBuf::from(&self.path),
            access: if self.read_only {
                Access::ReadOnly
            } else {
                Access::ReadWrite
            },
        }
    }
}

/// The root directory's identity, as `stat` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedRoot {
    /// The device number.
    pub device: u64,
    /// The inode number.
    pub inode: u64,
}

/// Everything the backend keeps about one sandbox.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BubblewrapData {
    /// The payload version.
    pub version: u32,
    /// The bwrap process the tool started.
    pub launcher_pid: u32,
    /// The pid of the sandbox's pid 1.
    pub sandbox_pid: u32,
    /// The launcher's start time.
    pub launcher_start: u64,
    /// The sandbox's start time.
    pub sandbox_start: u64,
    /// The seven namespace inodes.
    pub namespaces: [u64; 7],
    /// The uid the sandbox was created under.
    pub uid: u32,
    /// The sandbox root's identity.
    pub root: RecordedRoot,
    /// The host paths the sandbox was given.
    pub mounts: Vec<RecordedMount>,
    /// The host directory bound at the workspace path.
    pub workspace_host: String,
    /// The path the sandbox sees the workspace at.
    pub workspace: String,
    /// The scope unit holding the sandbox, when there is one.
    pub scope: Option<String>,
    /// Whether the sandbox is paused.
    pub paused: bool,
    /// Where the environment came from and what was withheld.
    pub environment: Provenance,
    /// Whether the sandbox shares the host network.
    pub network: Network,
    /// Ports the sandbox reaches on the host, when it shares the host network.
    pub forwards: Vec<crate::sandbox::contract::Forward>,
}

impl BubblewrapData {
    /// Read the payload out of a record, refusing a payload it cannot honour.
    pub fn from_record(record: &SandboxRecord) -> Result<Self, Error> {
        #[derive(Deserialize)]
        struct VersionOnly {
            version: u32,
        }

        let probe: VersionOnly =
            record
                .data
                .clone()
                .try_into()
                .map_err(|error: toml::de::Error| Error::Unavailable {
                    backend: BACKEND.to_string(),
                    reason: format!(
                        "sandbox '{}' has no readable payload version: {error}",
                        record.name
                    ),
                    remedy: "delete and recreate the sandbox".to_string(),
                })?;

        if probe.version > VERSION {
            return Err(Error::failed(format!(
                "sandbox '{}' was written by a newer release (payload version {})",
                record.name, probe.version
            )));
        }
        if probe.version < VERSION {
            return Err(Error::Unavailable {
                backend: BACKEND.to_string(),
                reason: format!(
                    "sandbox '{}' uses payload version {}, which this release does not read",
                    record.name, probe.version
                ),
                remedy: "delete and recreate the sandbox".to_string(),
            });
        }

        record
            .data
            .clone()
            .try_into()
            .map_err(|error: toml::de::Error| Error::Unavailable {
                backend: BACKEND.to_string(),
                reason: format!(
                    "sandbox '{}' has an unreadable payload: {error}",
                    record.name
                ),
                remedy: "delete and recreate the sandbox".to_string(),
            })
    }

    /// Serialise the payload for a record.
    pub fn into_value(self) -> Result<toml::Value, Error> {
        toml::Value::try_from(self)
            .map_err(|error| Error::failed(format!("could not serialise the record: {error}")))
    }

    /// The mount entries this payload records.
    pub fn mounts(&self) -> Vec<Mount> {
        self.mounts.iter().map(RecordedMount::mount).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{BubblewrapData, RecordedMount, RecordedRoot, VERSION};
    use crate::backends::bubblewrap::host::NamespaceInodes;
    use crate::backends::bubblewrap::view::{Access, Mount, Pinned, View, WORKSPACE};
    use crate::sandbox::contract::{Error, SandboxRecord};
    use crate::sandbox::environment::Provenance;

    fn pinned(path: &str, read_only: bool) -> Pinned {
        let file = std::fs::File::open("/dev/null").expect("an open file");
        Pinned {
            mount: Mount {
                path: path.into(),
                access: if read_only {
                    Access::ReadOnly
                } else {
                    Access::ReadWrite
                },
            },
            file,
        }
    }

    fn view() -> View {
        View {
            pinned: vec![
                pinned("/nix/store/abc", true),
                pinned("/nix/store/def", false),
            ],
        }
    }

    fn namespaces() -> NamespaceInodes {
        NamespaceInodes {
            user: 1,
            ipc: 2,
            net: 3,
            pid: 4,
            mnt: 5,
            uts: 6,
            cgroup: 7,
        }
    }

    fn data() -> BubblewrapData {
        let view = view();
        BubblewrapData {
            version: VERSION,
            launcher_pid: 10,
            launcher_start: 100,
            sandbox_pid: 11,
            sandbox_start: 101,
            namespaces: namespaces().entries().map(|(_, inode)| inode),
            uid: 1000,
            root: RecordedRoot {
                device: 8,
                inode: 9,
            },
            mounts: view
                .pinned
                .iter()
                .map(|pinned| RecordedMount::from(&pinned.mount))
                .collect(),
            workspace_host: "/state/workspace".to_string(),
            workspace: WORKSPACE.to_string(),
            scope: Some("litterbox-demo".to_string()),
            paused: false,
            environment: Provenance {
                origin: crate::sandbox::environment::Origin::Default,
                variables: Vec::new(),
                host_paths: Vec::new(),
                steps: Vec::new(),
            },
            forwards: Vec::new(),
            network: crate::backends::bubblewrap::backend::Network::Denied,
        }
    }

    fn record(data: &BubblewrapData) -> SandboxRecord {
        SandboxRecord {
            name: "demo".to_string(),
            backend: "bubblewrap".to_string(),
            project_slug: "project".to_string(),
            data: toml::Value::try_from(data).expect("payload"),
        }
    }

    #[test]
    fn a_payload_round_trips() {
        let data = data();
        let read = BubblewrapData::from_record(&record(&data)).expect("readable");
        assert_eq!(read, data);
        assert_eq!(read.workspace, WORKSPACE);
        assert_eq!(read.namespaces, [1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(read.mounts().len(), 2);
        assert_eq!(read.mounts()[1].access, Access::ReadWrite);
    }

    #[test]
    fn a_newer_payload_is_unreadable() {
        let mut data = data();
        data.version = VERSION + 1;
        let error = BubblewrapData::from_record(&record(&data)).expect_err("refused");
        assert!(error.to_string().contains("newer release"), "{error}");
    }

    #[test]
    fn an_older_payload_is_unavailable_with_a_remedy() {
        let mut data = data();
        data.version = VERSION - 1;
        match BubblewrapData::from_record(&record(&data)) {
            Err(Error::Unavailable { reason, remedy, .. }) => {
                assert!(reason.contains("payload version"), "{reason}");
                assert!(remedy.contains("delete and recreate"), "{remedy}");
            }
            other => panic!("expected an unavailable report, got {other:?}"),
        }
    }

    #[test]
    fn a_payload_without_a_version_is_unavailable() {
        let record = SandboxRecord {
            name: "demo".to_string(),
            backend: "bubblewrap".to_string(),
            project_slug: "project".to_string(),
            data: toml::from_str("sandbox_pid = 12").expect("payload"),
        };
        match BubblewrapData::from_record(&record) {
            Err(Error::Unavailable { remedy, .. }) => {
                assert!(remedy.contains("delete and recreate"), "{remedy}");
            }
            other => panic!("expected an unavailable report, got {other:?}"),
        }
    }

    #[test]
    fn a_payload_missing_a_field_is_unavailable() {
        let mut value = toml::Value::try_from(data()).expect("payload");
        if let Some(table) = value.as_table_mut() {
            table.remove("sandbox_pid");
        }
        let record = SandboxRecord {
            name: "demo".to_string(),
            backend: "bubblewrap".to_string(),
            project_slug: "project".to_string(),
            data: value,
        };
        assert!(matches!(
            BubblewrapData::from_record(&record),
            Err(Error::Unavailable { .. })
        ));
    }
}
