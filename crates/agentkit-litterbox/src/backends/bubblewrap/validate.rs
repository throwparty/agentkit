//! Deciding whether a record still describes a live sandbox.
//!
//! One function answers for every operation. It reports what it found rather
//! than an error, because a missing sandbox is a failure for a command and a
//! normal state for inspection and teardown.

use std::collections::BTreeSet;
use std::path::PathBuf;

use super::host::{Host, NamespaceInodes};
use super::payload::BubblewrapData;
use super::view::WORKSPACE;
use crate::sandbox::contract::{Error, SandboxRecord};

/// What validation found.
#[derive(Debug)]
pub enum Verdict {
    /// The record describes a live sandbox.
    Live(Box<BubblewrapData>),
    /// The sandbox process is gone.
    Absent,
    /// A live process does not match the record.
    Forbidden {
        /// Why it was refused.
        reason: String,
    },
    /// The payload itself cannot be read or is from another edition.
    Editions(Error),
}

#[cfg(test)]
impl Verdict {
    /// The payload, or the failure a command should report.
    pub fn require(self, name: &str) -> Result<BubblewrapData, Error> {
        match self {
            Self::Live(data) => Ok(*data),
            Self::Absent => Err(Error::NotFound {
                name: name.to_string(),
            }),
            Self::Forbidden { reason } => Err(Error::Denied {
                operation: "use the sandbox".to_string(),
                reason,
            }),
            Self::Editions(error) => Err(error),
        }
    }

    /// Whether the sandbox process is gone.
    pub fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }

    /// Whether the record is live.
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live(_))
    }
}

/// Validate a record against the host.
pub fn validate(record: &SandboxRecord, host: &dyn Host) -> Verdict {
    let data = match BubblewrapData::from_record(record) {
        Ok(data) => data,
        Err(error) => return Verdict::Editions(error),
    };

    let Some(process) = host.process(data.sandbox_pid) else {
        return Verdict::Absent;
    };
    if process.state == super::host::ProcessState::Zombie {
        return Verdict::Absent;
    }
    if process.uid != data.uid {
        return Verdict::Forbidden {
            reason: format!(
                "sandbox '{}' runs as uid {} but the record was written by uid {}",
                record.name, process.uid, data.uid
            ),
        };
    }
    if process.start_time != data.sandbox_start {
        return Verdict::Forbidden {
            reason: format!(
                "sandbox '{}' pid {} was reused by another process",
                record.name, data.sandbox_pid
            ),
        };
    }

    let Some(namespaces) = host.namespaces(data.sandbox_pid) else {
        return Verdict::Absent;
    };
    if let Err(reason) = compare_namespaces(&namespaces, &data.namespaces) {
        return Verdict::Forbidden { reason };
    }

    let Some(root) = host.root_identity(data.sandbox_pid) else {
        return Verdict::Absent;
    };
    if root.device != data.root.device || root.inode != data.root.inode {
        return Verdict::Forbidden {
            reason: format!("sandbox '{}' root is not the one recorded", record.name),
        };
    }

    let Some(mounts) = host.mounts(data.sandbox_pid) else {
        return Verdict::Absent;
    };
    let recorded = data.mounts();
    for mount in &recorded {
        let Some(entry) = mounts.iter().find(|entry| entry.path == mount.path) else {
            return Verdict::Forbidden {
                reason: format!(
                    "sandbox '{}' no longer mounts {}",
                    record.name,
                    mount.path.display()
                ),
            };
        };
        if entry.read_only != (mount.access == super::view::Access::ReadOnly) {
            return Verdict::Forbidden {
                reason: format!(
                    "sandbox '{}' mounts {} with the wrong access",
                    record.name,
                    mount.path.display()
                ),
            };
        }
    }

    let allowed: BTreeSet<PathBuf> = recorded
        .iter()
        .map(|mount| mount.path.clone())
        .chain([PathBuf::from(WORKSPACE)])
        .collect();
    for entry in &mounts {
        if entry.is_host_mount() && !allowed.contains(&entry.path) {
            return Verdict::Forbidden {
                reason: format!(
                    "sandbox '{}' mounts {}, which the record does not name",
                    record.name,
                    entry.path.display()
                ),
            };
        }
    }

    Verdict::Live(Box::new(data))
}

fn compare_namespaces(live: &NamespaceInodes, recorded: &[u64; 7]) -> Result<(), String> {
    for ((name, inode), expected) in live.entries().iter().zip(recorded) {
        if inode != expected {
            return Err(format!("sandbox {name} namespace is not the one recorded"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Verdict, validate};
    use crate::backends::bubblewrap::host::{
        FakeHost, MountEntry, NamespaceInodes, ProcessInfo, ProcessState, RootIdentity,
    };
    use crate::backends::bubblewrap::payload::{
        BubblewrapData, RecordedMount, RecordedRoot, VERSION,
    };
    use crate::backends::bubblewrap::view::WORKSPACE;
    use crate::sandbox::contract::{Error, SandboxRecord};
    use crate::sandbox::environment::{Origin, Provenance};

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
            mounts: vec![RecordedMount {
                path: "/nix/store/abc".to_string(),
                read_only: true,
            }],
            workspace_host: "/state/workspace".to_string(),
            workspace: WORKSPACE.to_string(),
            scope: None,
            paused: false,
            environment: Provenance {
                origin: Origin::Default,
                variables: Vec::new(),
                host_paths: Vec::new(),
                steps: Vec::new(),
            },
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

    fn host(data: &BubblewrapData) -> FakeHost {
        let mut host = FakeHost::default();
        host.processes.insert(
            data.sandbox_pid,
            ProcessInfo {
                state: ProcessState::Waiting,
                start_time: data.sandbox_start,
                uid: data.uid,
            },
        );
        host.namespaces.insert(data.sandbox_pid, namespaces());
        host.roots.insert(
            data.sandbox_pid,
            RootIdentity {
                device: data.root.device,
                inode: data.root.inode,
            },
        );
        host.mounts.insert(
            data.sandbox_pid,
            vec![
                MountEntry {
                    path: PathBuf::from("/nix/store/abc"),
                    read_only: true,
                    fstype: "ext4".to_string(),
                },
                MountEntry {
                    path: PathBuf::from(WORKSPACE),
                    read_only: false,
                    fstype: "ext4".to_string(),
                },
                MountEntry {
                    path: PathBuf::from("/proc"),
                    read_only: false,
                    fstype: "proc".to_string(),
                },
            ],
        );
        host
    }

    #[test]
    fn a_live_record_is_live() {
        let data = data();
        assert!(validate(&record(&data), &host(&data)).is_live());
    }

    #[test]
    fn a_missing_process_is_absent() {
        let data = data();
        let host = FakeHost::default();
        assert!(validate(&record(&data), &host).is_absent());
    }

    #[test]
    fn a_zombie_is_absent() {
        let data = data();
        let mut host = host(&data);
        host.processes
            .get_mut(&data.sandbox_pid)
            .expect("process")
            .state = ProcessState::Zombie;
        assert!(validate(&record(&data), &host).is_absent());
    }

    #[test]
    fn a_reused_pid_is_forbidden() {
        let data = data();
        let mut host = host(&data);
        host.processes
            .get_mut(&data.sandbox_pid)
            .expect("process")
            .start_time = data.sandbox_start + 1;
        match validate(&record(&data), &host) {
            Verdict::Forbidden { reason } => assert!(reason.contains("reused"), "{reason}"),
            other => panic!("expected forbidden, got {other:?}"),
        }
    }

    #[test]
    fn a_different_uid_is_forbidden() {
        let data = data();
        let mut host = host(&data);
        host.processes
            .get_mut(&data.sandbox_pid)
            .expect("process")
            .uid = 0;
        match validate(&record(&data), &host) {
            Verdict::Forbidden { reason } => assert!(reason.contains("uid"), "{reason}"),
            other => panic!("expected forbidden, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_namespace_is_forbidden() {
        let data = data();
        let mut host = host(&data);
        host.namespaces
            .get_mut(&data.sandbox_pid)
            .expect("namespaces")
            .pid += 1;
        match validate(&record(&data), &host) {
            Verdict::Forbidden { reason } => assert!(reason.contains("pid"), "{reason}"),
            other => panic!("expected forbidden, got {other:?}"),
        }
    }

    #[test]
    fn a_different_root_is_forbidden() {
        let data = data();
        let mut host = host(&data);
        host.roots.get_mut(&data.sandbox_pid).expect("root").inode += 1;
        assert!(matches!(
            validate(&record(&data), &host),
            Verdict::Forbidden { .. }
        ));
    }

    #[test]
    fn a_missing_recorded_mount_is_forbidden() {
        let data = data();
        let mut host = host(&data);
        host.mounts
            .get_mut(&data.sandbox_pid)
            .expect("mounts")
            .remove(0);
        match validate(&record(&data), &host) {
            Verdict::Forbidden { reason } => {
                assert!(reason.contains("no longer mounts"), "{reason}")
            }
            other => panic!("expected forbidden, got {other:?}"),
        }
    }

    #[test]
    fn an_unrecorded_host_mount_is_forbidden() {
        let data = data();
        let mut host = host(&data);
        host.mounts
            .get_mut(&data.sandbox_pid)
            .expect("mounts")
            .push(MountEntry {
                path: PathBuf::from("/home/someone"),
                read_only: true,
                fstype: "ext4".to_string(),
            });
        match validate(&record(&data), &host) {
            Verdict::Forbidden { reason } => {
                assert!(reason.contains("does not name"), "{reason}");
            }
            other => panic!("expected forbidden, got {other:?}"),
        }
    }

    #[test]
    fn the_wrong_mount_access_is_forbidden() {
        let data = data();
        let mut host = host(&data);
        host.mounts.get_mut(&data.sandbox_pid).expect("mounts")[0].read_only = false;
        assert!(matches!(
            validate(&record(&data), &host),
            Verdict::Forbidden { .. }
        ));
    }

    #[test]
    fn an_unreadable_payload_is_an_edition_failure() {
        let data = data();
        let mut data = data;
        data.version = VERSION + 1;
        match validate(&record(&data), &host(&data)) {
            Verdict::Editions(Error::Failed { .. }) => {}
            other => panic!("expected an edition failure, got {other:?}"),
        }
    }

    #[test]
    fn require_maps_an_absent_sandbox_to_not_found() {
        let error = Verdict::Absent.require("demo").expect_err("absent");
        assert!(matches!(error, Error::NotFound { .. }));
    }
}
