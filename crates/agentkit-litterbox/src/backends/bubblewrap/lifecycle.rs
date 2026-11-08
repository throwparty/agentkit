//! Pausing, resuming and bounding a sandbox.
//!
//! With a scope, freeze covers every member atomically and is confirmed by
//! watching CPU time stop. Without one, pause walks the pid namespace with
//! stop-and-rescan until no new member appears.

use std::time::Duration;

use super::BACKEND;
use super::host::{Host, ProcessState};
use super::payload::BubblewrapData;
use crate::sandbox::contract::Error;
use crate::sandbox::facets::ResourceLimits;

/// How long a freeze has to show no progress.
const FREEZE_TIMEOUT: Duration = Duration::from_secs(5);

/// How many rescan rounds a namespace pause may take.
const PAUSE_ROUNDS: usize = 10;

/// Pause the sandbox.
pub async fn pause(host: &dyn Host, data: &BubblewrapData) -> Result<(), Error> {
    if let Some(unit) = &data.scope {
        return pause_scope(host, unit, data.sandbox_pid).await;
    }
    pause_namespace(host, data.sandbox_pid)
}

async fn pause_scope(host: &dyn Host, unit: &str, pid: u32) -> Result<(), Error> {
    host.write_scope(unit, "cgroup.freeze", "1")
        .map_err(|error| Error::failed(format!("the sandbox could not be frozen: {error}")))?;

    let before = host.cpu_time(pid).unwrap_or_default();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let started = std::time::Instant::now();
    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let now = host.cpu_time(pid).unwrap_or_default();
        if now == before {
            return Ok(());
        }
        if started.elapsed() >= FREEZE_TIMEOUT {
            let _ = host.write_scope(unit, "cgroup.freeze", "0");
            return Err(Error::failed(
                "the sandbox kept making progress after it was frozen",
            ));
        }
        // The CPU counter can only be compared between samples; keep the
        // baseline and wait for a window where it does not move.
        if started.elapsed() > Duration::from_millis(500) && now == before {
            return Ok(());
        }
    }
}

fn pause_namespace(host: &dyn Host, pid: u32) -> Result<(), Error> {
    for _ in 0..PAUSE_ROUNDS {
        let Some(members) = host.namespace_members(pid) else {
            return Err(Error::failed("the sandbox namespace cannot be listed"));
        };
        let mut stopped = 0;
        for member in &members {
            // SAFETY: signalling a member of the sandbox is the point.
            if unsafe { libc::kill(*member as i32, libc::SIGSTOP) } == 0 {
                stopped += 1;
            }
        }
        let Some(after) = host.namespace_members(pid) else {
            return Err(Error::failed("the sandbox namespace cannot be listed"));
        };
        let running: Vec<u32> = after
            .into_iter()
            .filter(|member| {
                host.process(*member)
                    .is_some_and(|process| process.state != ProcessState::Stopped)
            })
            .collect();
        if running.is_empty() {
            return Ok(());
        }
        let _ = stopped;
    }
    Err(Error::failed(
        "the sandbox kept forking while it was being paused",
    ))
}

/// Resume the sandbox.
pub fn resume(host: &dyn Host, data: &BubblewrapData) -> Result<(), Error> {
    if let Some(unit) = &data.scope {
        return host
            .write_scope(unit, "cgroup.freeze", "0")
            .map_err(|error| Error::failed(format!("the sandbox could not be thawed: {error}")));
    }
    let Some(members) = host.namespace_members(data.sandbox_pid) else {
        return Err(Error::failed("the sandbox namespace cannot be listed"));
    };
    for member in members {
        // SAFETY: signalling a member of the sandbox is the point.
        unsafe { libc::kill(member as i32, libc::SIGCONT) };
    }
    Ok(())
}

/// Whether the sandbox is paused, as the mechanism can observe it.
pub fn is_paused(host: &dyn Host, data: &BubblewrapData) -> bool {
    if let Some(unit) = &data.scope {
        return host
            .read_scope(unit, "cgroup.freeze")
            .is_some_and(|value| value.trim() == "1");
    }
    let Some(members) = host.namespace_members(data.sandbox_pid) else {
        return false;
    };
    !members.is_empty()
        && members.iter().all(|member| {
            host.process(*member)
                .is_some_and(|process| process.state == ProcessState::Stopped)
        })
}

/// Apply resource bounds to the scope.
pub fn set_limits(
    host: &dyn Host,
    data: &BubblewrapData,
    limits: &ResourceLimits,
) -> Result<(), Error> {
    let unit = data.scope.as_deref().ok_or_else(|| Error::Unsupported {
        operation: "bound the sandbox".to_string(),
        backend: BACKEND.to_string(),
        primitive: "a cgroup v2 user scope".to_string(),
        reason: "the sandbox was created without a scope, so it has no cgroup to bound".to_string(),
        remedy: "run on a host with a delegating user manager and create again".to_string(),
    })?;
    let memory = limits
        .memory_bytes
        .map(|bytes| bytes.to_string())
        .unwrap_or_else(|| "max".to_string());
    host.write_scope(unit, "memory.max", &memory)
        .map_err(|error| Error::failed(format!("the memory bound was not applied: {error}")))?;
    let tasks = limits
        .tasks
        .map(|tasks| tasks.to_string())
        .unwrap_or_else(|| "max".to_string());
    host.write_scope(unit, "pids.max", &tasks)
        .map_err(|error| Error::failed(format!("the task bound was not applied: {error}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::{is_paused, pause, resume, set_limits};
    use crate::backends::bubblewrap::host::{
        FakeHost, MountEntry, NamespaceInodes, ProcessInfo, ProcessState, RootIdentity,
    };
    use crate::backends::bubblewrap::payload::BubblewrapData;
    use crate::backends::bubblewrap::payload::{RecordedRoot, VERSION};
    use crate::sandbox::environment::{Origin, Provenance};
    use crate::sandbox::facets::ResourceLimits;

    fn data(scope: Option<&str>, pid: u32) -> BubblewrapData {
        BubblewrapData {
            version: VERSION,
            launcher_pid: pid + 1,
            launcher_start: 1,
            sandbox_pid: pid,
            sandbox_start: 1,
            namespaces: [1; 7],
            uid: 1000,
            root: RecordedRoot {
                device: 1,
                inode: 2,
            },
            mounts: Vec::new(),
            workspace_host: "/state/workspace".to_string(),
            workspace: "/src".to_string(),
            scope: scope.map(str::to_string),
            paused: false,
            environment: Provenance {
                origin: Origin::Default,
                variables: Vec::new(),
                host_paths: Vec::new(),
                steps: Vec::new(),
            },
        }
    }

    fn host(pid: u32, state: ProcessState) -> FakeHost {
        let mut host = FakeHost::default();
        host.processes.insert(
            pid,
            ProcessInfo {
                state,
                start_time: 1,
                uid: 1000,
            },
        );
        host.namespaces.insert(
            pid,
            NamespaceInodes {
                user: 1,
                ipc: 2,
                net: 3,
                pid: 4,
                mnt: 5,
                uts: 6,
                cgroup: 7,
            },
        );
        host.roots.insert(
            pid,
            RootIdentity {
                device: 1,
                inode: 2,
            },
        );
        host.mounts.insert(
            pid,
            vec![MountEntry {
                path: PathBuf::from("/src"),
                read_only: false,
                fstype: "ext4".to_string(),
            }],
        );
        host
    }

    #[tokio::test]
    async fn a_scoped_sandbox_is_paused_by_the_freeze_file() {
        let mut host = host(11, ProcessState::Waiting);
        host.scopes.insert(
            ("unit".to_string(), "cgroup.freeze".to_string()),
            "0".to_string(),
        );
        let data = data(Some("unit"), 11);
        assert!(!is_paused(&host, &data));
        pause(&host, &data).await.expect("paused");
        resume(&host, &data).expect("resumed");
    }

    #[tokio::test]
    async fn an_unscoped_sandbox_is_paused_by_signal() {
        let mut host = host(11, ProcessState::Waiting);
        host.processes.get_mut(&11).expect("process").state = ProcessState::Stopped;
        let data = data(None, 11);
        assert!(is_paused(&host, &data));
        resume(&host, &data).expect("resumed");
    }

    #[tokio::test]
    async fn a_pause_without_a_scope_reports_the_state_it_finds() {
        let host = host(11, ProcessState::Waiting);
        let data = data(None, 11);
        assert!(!is_paused(&host, &data));
    }

    #[test]
    fn limits_need_a_scope() {
        let host = host(11, ProcessState::Waiting);
        let data = data(None, 11);
        let error = set_limits(&host, &data, &ResourceLimits::default()).expect_err("refused");
        assert!(matches!(
            error,
            crate::sandbox::contract::Error::Unsupported { .. }
        ));
    }

    #[test]
    fn limits_are_written_to_the_scope() {
        let mut host = host(11, ProcessState::Waiting);
        host.scopes.insert(
            ("unit".to_string(), "memory.max".to_string()),
            "max".to_string(),
        );
        host.scopes.insert(
            ("unit".to_string(), "pids.max".to_string()),
            "max".to_string(),
        );
        let data = data(Some("unit"), 11);
        set_limits(
            &host,
            &data,
            &ResourceLimits {
                memory_bytes: Some(1024),
                tasks: Some(10),
            },
        )
        .expect("applied");
    }

    #[test]
    fn an_empty_member_list_is_not_paused() {
        let host = FakeHost {
            processes: HashMap::new(),
            ..FakeHost::default()
        };
        assert!(!is_paused(&host, &data(None, 11)));
    }
}
