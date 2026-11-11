//! The host reads and writes the backend needs, behind one trait.
//!
//! Everything the backend learns about a sandbox process comes through here,
//! so a unit test can answer from a table instead of starting a sandbox.

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What a process is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessState {
    /// Running or runnable.
    Running,
    /// Sleeping or waiting.
    Waiting,
    /// Exited and waiting to be reaped.
    Zombie,
    /// Stopped by a signal.
    Stopped,
}

/// What the backend needs to know about one process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessInfo {
    /// The process's state.
    pub state: ProcessState,
    /// The process's start time in clock ticks since boot.
    pub start_time: u64,
    /// The real uid the process runs as.
    pub uid: u32,
}

/// The seven namespaces, in the order every record and descriptor list uses.
///
/// The payload stores the inodes as an array, so the order is part of the
/// record format. This is the one place it is written down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Namespace {
    /// The user namespace.
    User = 0,
    /// The ipc namespace.
    Ipc = 1,
    /// The network namespace.
    Net = 2,
    /// The pid namespace.
    Pid = 3,
    /// The mount namespace.
    Mnt = 4,
    /// The uts namespace.
    Uts = 5,
    /// The cgroup namespace.
    Cgroup = 6,
}

impl Namespace {
    /// Every namespace, in record order.
    pub const ALL: [Namespace; 7] = [
        Namespace::User,
        Namespace::Ipc,
        Namespace::Net,
        Namespace::Pid,
        Namespace::Mnt,
        Namespace::Uts,
        Namespace::Cgroup,
    ];

    /// The position the namespace takes in a recorded array or descriptor list.
    pub fn index(self) -> usize {
        self as usize
    }

    /// The name the kernel uses for it, as it appears in `/proc`.
    pub fn name(self) -> &'static str {
        match self {
            Namespace::User => "user",
            Namespace::Ipc => "ipc",
            Namespace::Net => "net",
            Namespace::Pid => "pid",
            Namespace::Mnt => "mnt",
            Namespace::Uts => "uts",
            Namespace::Cgroup => "cgroup",
        }
    }
}

/// The seven namespace inodes a sandbox process holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamespaceInodes {
    /// The user namespace.
    pub user: u64,
    /// The ipc namespace.
    pub ipc: u64,
    /// The network namespace.
    pub net: u64,
    /// The pid namespace.
    pub pid: u64,
    /// The mount namespace.
    pub mnt: u64,
    /// The uts namespace.
    pub uts: u64,
    /// The cgroup namespace.
    pub cgroup: u64,
}

impl NamespaceInodes {
    /// The inodes in the order the join enters them.
    pub fn entries(&self) -> [(&'static str, u64); 7] {
        [
            (Namespace::User.name(), self.user),
            (Namespace::Ipc.name(), self.ipc),
            (Namespace::Net.name(), self.net),
            (Namespace::Pid.name(), self.pid),
            (Namespace::Mnt.name(), self.mnt),
            (Namespace::Uts.name(), self.uts),
            (Namespace::Cgroup.name(), self.cgroup),
        ]
    }

    /// The inode recorded for one namespace.
    pub fn inode(&self, namespace: Namespace) -> u64 {
        match namespace {
            Namespace::User => self.user,
            Namespace::Ipc => self.ipc,
            Namespace::Net => self.net,
            Namespace::Pid => self.pid,
            Namespace::Mnt => self.mnt,
            Namespace::Uts => self.uts,
            Namespace::Cgroup => self.cgroup,
        }
    }
}

impl From<[u64; 7]> for NamespaceInodes {
    fn from(inodes: [u64; 7]) -> Self {
        Self {
            user: inodes[Namespace::User.index()],
            ipc: inodes[Namespace::Ipc.index()],
            net: inodes[Namespace::Net.index()],
            pid: inodes[Namespace::Pid.index()],
            mnt: inodes[Namespace::Mnt.index()],
            uts: inodes[Namespace::Uts.index()],
            cgroup: inodes[Namespace::Cgroup.index()],
        }
    }
}

impl From<&NamespaceInodes> for [u64; 7] {
    fn from(namespaces: &NamespaceInodes) -> Self {
        Namespace::ALL.map(|namespace| namespaces.inode(namespace))
    }
}

/// One entry of a process's mount table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountEntry {
    /// Where the mount appears.
    pub path: PathBuf,
    /// Whether the mount is read-only.
    pub read_only: bool,
    /// The file system type.
    pub fstype: String,
}

impl MountEntry {
    /// Whether this mount is a host path rather than one the mechanism made.
    pub fn is_host_mount(&self) -> bool {
        !matches!(
            self.fstype.as_str(),
            "tmpfs" | "devtmpfs" | "devpts" | "proc" | "sysfs" | "mqueue" | "cgroup2" | "overlay"
        )
    }
}

/// The device and inode of a process's root directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RootIdentity {
    /// The device number.
    pub device: u64,
    /// The inode number.
    pub inode: u64,
}

/// The host reads and writes the backend needs.
pub trait Host: Send + Sync {
    /// A process's state, start time and uid.
    fn process(&self, pid: u32) -> Option<ProcessInfo>;
    /// The seven namespace inodes a process holds.
    fn namespaces(&self, pid: u32) -> Option<NamespaceInodes>;
    /// The device and inode of a process's root directory.
    fn root_identity(&self, pid: u32) -> Option<RootIdentity>;
    /// A process's mount table.
    fn mounts(&self, pid: u32) -> Option<Vec<MountEntry>>;
    /// A process's environment, split into name and value.
    fn environment(&self, pid: u32) -> Option<Vec<(String, String)>>;
    /// Resolve a binary name on the host PATH, following links to the file.
    fn resolve(&self, name: &str) -> Option<PathBuf>;
    /// The owner and mode bits of a path.
    fn ownership(&self, path: &Path) -> Option<(u32, u32)>;
    /// The cgroup directory for a user scope unit.
    fn scope_directory(&self, unit: &str) -> Option<PathBuf>;
    /// Read a file in a scope's cgroup directory.
    fn read_scope(&self, unit: &str, file: &str) -> Option<String>;
    /// The uid the caller runs as.
    fn effective_uid(&self) -> u32;
    /// Write a file in a scope's cgroup directory.
    fn write_scope(&self, unit: &str, file: &str, value: &str) -> std::io::Result<()>;
    /// Every process in the namespace the given pid belongs to.
    fn namespace_members(&self, pid: u32) -> Option<Vec<u32>>;
    /// Whether a delegating user manager can give this process a scope.
    fn user_scope(&self) -> bool;
    /// Every process whose pid namespace is the given inode, whether or not
    /// the pid that named it still exists.
    fn namespace_members_of(&self, inode: u64) -> Vec<u32>;
}

/// The host this process is running on.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemHost;

impl SystemHost {
    /// The system host.
    pub fn new() -> Self {
        Self
    }

    fn stat(pid: u32) -> Option<(ProcessState, u64)> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // The command name is parenthesised and may contain spaces, so split
        // after the last closing parenthesis.
        let close = text.rfind(')')?;
        let rest: Vec<&str> = text[close + 1..].split_whitespace().collect();
        let state = match *rest.first()? {
            "R" => ProcessState::Running,
            "Z" => ProcessState::Zombie,
            "T" | "t" => ProcessState::Stopped,
            _ => ProcessState::Waiting,
        };
        // Field 22 in the whole line; three fields precede the state field.
        let start_time = rest.get(19)?.parse().ok()?;
        Some((state, start_time))
    }

    fn uid(pid: u32) -> Option<u32> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("Uid:") {
                return rest.split_whitespace().next()?.parse().ok();
            }
        }
        None
    }

    fn namespace(pid: u32, name: &str) -> Option<u64> {
        let link = std::fs::read_link(format!("/proc/{pid}/ns/{name}")).ok()?;
        let text = link.to_string_lossy();
        let start = text.find('[')? + 1;
        let end = text.find(']')?;
        text[start..end].parse().ok()
    }
}

impl Host for SystemHost {
    fn process(&self, pid: u32) -> Option<ProcessInfo> {
        let (state, start_time) = Self::stat(pid)?;
        let uid = Self::uid(pid)?;
        Some(ProcessInfo {
            state,
            start_time,
            uid,
        })
    }

    fn namespaces(&self, pid: u32) -> Option<NamespaceInodes> {
        Some(NamespaceInodes {
            user: Self::namespace(pid, "user")?,
            ipc: Self::namespace(pid, "ipc")?,
            net: Self::namespace(pid, "net")?,
            pid: Self::namespace(pid, "pid")?,
            mnt: Self::namespace(pid, "mnt")?,
            uts: Self::namespace(pid, "uts")?,
            cgroup: Self::namespace(pid, "cgroup")?,
        })
    }

    fn root_identity(&self, pid: u32) -> Option<RootIdentity> {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(format!("/proc/{pid}/root")).ok()?;
        Some(RootIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn mounts(&self, pid: u32) -> Option<Vec<MountEntry>> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/mountinfo")).ok()?;
        let mut entries = Vec::new();
        for line in text.lines() {
            let (left, right) = line.split_once(" - ")?;
            let fields: Vec<&str> = left.split_whitespace().collect();
            if fields.len() < 6 {
                continue;
            }
            let fstype = right.split_whitespace().next().unwrap_or("");
            entries.push(MountEntry {
                path: PathBuf::from(fields[4]),
                read_only: fields[5].split(',').any(|option| option == "ro"),
                fstype: fstype.to_string(),
            });
        }
        Some(entries)
    }

    fn environment(&self, pid: u32) -> Option<Vec<(String, String)>> {
        let bytes = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
        let mut variables = Vec::new();
        for entry in bytes.split(|byte| *byte == 0) {
            if entry.is_empty() {
                continue;
            }
            let text = String::from_utf8_lossy(entry);
            if let Some((name, value)) = text.split_once('=') {
                variables.push((name.to_string(), value.to_string()));
            }
        }
        Some(variables)
    }

    fn resolve(&self, name: &str) -> Option<PathBuf> {
        let candidate = if name.contains('/') {
            PathBuf::from(name)
        } else {
            let path = std::env::var_os("PATH")?;
            std::env::split_paths(&path)
                .map(|directory| directory.join(name))
                .find(|candidate| candidate.is_file())?
        };
        // Resolving to the target lets the view pin it without following a
        // link, and lets the ownership check look at the file that runs.
        std::fs::canonicalize(candidate)
            .ok()
            .filter(|path| path.is_file())
    }

    fn ownership(&self, path: &Path) -> Option<(u32, u32)> {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(path).ok()?;
        Some((metadata.uid(), metadata.mode()))
    }

    fn scope_directory(&self, unit: &str) -> Option<PathBuf> {
        let output = std::process::Command::new("systemctl")
            .args([
                "--user",
                "show",
                &format!("{unit}.scope"),
                "-p",
                "ControlGroup",
                "--value",
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let control_group = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if control_group.is_empty() {
            return None;
        }
        Some(Path::new("/sys/fs/cgroup").join(control_group.trim_start_matches('/')))
    }

    fn read_scope(&self, unit: &str, file: &str) -> Option<String> {
        let directory = self.scope_directory(unit)?;
        std::fs::read_to_string(directory.join(file)).ok()
    }

    fn write_scope(&self, unit: &str, file: &str, value: &str) -> std::io::Result<()> {
        let directory = self
            .scope_directory(unit)
            .ok_or_else(|| std::io::Error::other(format!("no scope named {unit}")))?;
        std::fs::write(directory.join(file), value)
    }

    fn effective_uid(&self) -> u32 {
        // SAFETY: geteuid has no preconditions and cannot fail.
        unsafe { libc::geteuid() }
    }

    fn namespace_members(&self, pid: u32) -> Option<Vec<u32>> {
        let namespace = std::fs::read_link(format!("/proc/{pid}/ns/pid")).ok()?;
        let mut members = Vec::new();
        for entry in std::fs::read_dir("/proc").ok()? {
            let entry = entry.ok()?;
            let name = entry.file_name();
            let Some(candidate) = name.to_str().and_then(|name| name.parse::<u32>().ok()) else {
                continue;
            };
            let Ok(link) = std::fs::read_link(format!("/proc/{candidate}/ns/pid")) else {
                continue;
            };
            if link == namespace {
                members.push(candidate);
            }
        }
        members.sort_unstable();
        Some(members)
    }

    fn user_scope(&self) -> bool {
        // A binary and a cgroup mount do not prove a usable user manager: on a
        // host with systemd installed but no user bus, `systemd-run --user`
        // fails even though both are present. Ask the manager itself.
        let Some(launcher) = self.resolve("systemd-run") else {
            return false;
        };
        std::process::Command::new(launcher)
            .args(["--user", "--scope", "--quiet", "--", "true"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn namespace_members_of(&self, inode: u64) -> Vec<u32> {
        let wanted = format!("pid:[{inode}]");
        let mut members = Vec::new();
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return members;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(candidate) = name.to_str().and_then(|name| name.parse::<u32>().ok()) else {
                continue;
            };
            let Ok(link) = std::fs::read_link(format!("/proc/{candidate}/ns/pid")) else {
                continue;
            };
            if link.as_os_str() == std::ffi::OsStr::new(&wanted) {
                members.push(candidate);
            }
        }
        members.sort_unstable();
        members
    }
}

/// The host, shared with a backend.
pub type SharedHost = Arc<dyn Host>;

/// A host that answers from a table, for tests.
#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub struct FakeHost {
    /// Processes by pid.
    pub processes: std::collections::HashMap<u32, ProcessInfo>,
    /// Namespace inodes by pid.
    pub namespaces: std::collections::HashMap<u32, NamespaceInodes>,
    /// Root identities by pid.
    pub roots: std::collections::HashMap<u32, RootIdentity>,
    /// Mount tables by pid.
    pub mounts: std::collections::HashMap<u32, Vec<MountEntry>>,
    /// Environments by pid.
    pub environments: std::collections::HashMap<u32, Vec<(String, String)>>,
    /// Binaries the host can resolve.
    pub binaries: std::collections::HashMap<String, PathBuf>,
    /// Owner and mode of each path, defaulting to a root-owned, unshared file.
    pub ownership: std::collections::HashMap<PathBuf, (u32, u32)>,
    /// Scope files, keyed by unit and file name.
    pub scopes: std::collections::HashMap<(String, String), String>,
    /// Whether a user manager answers for this host.
    pub user_scope: bool,
}

#[cfg(test)]
impl Host for FakeHost {
    fn process(&self, pid: u32) -> Option<ProcessInfo> {
        self.processes.get(&pid).cloned()
    }

    fn namespaces(&self, pid: u32) -> Option<NamespaceInodes> {
        self.namespaces.get(&pid).cloned()
    }

    fn root_identity(&self, pid: u32) -> Option<RootIdentity> {
        self.roots.get(&pid).copied()
    }

    fn mounts(&self, pid: u32) -> Option<Vec<MountEntry>> {
        self.mounts.get(&pid).cloned()
    }

    fn environment(&self, pid: u32) -> Option<Vec<(String, String)>> {
        self.environments.get(&pid).cloned()
    }

    fn resolve(&self, name: &str) -> Option<PathBuf> {
        self.binaries.get(name).cloned()
    }

    fn ownership(&self, path: &Path) -> Option<(u32, u32)> {
        self.ownership.get(path).copied()
    }

    fn scope_directory(&self, unit: &str) -> Option<PathBuf> {
        self.scopes
            .keys()
            .any(|(candidate, _)| candidate == unit)
            .then(|| PathBuf::from(format!("/sys/fs/cgroup/{unit}.scope")))
    }

    fn read_scope(&self, unit: &str, file: &str) -> Option<String> {
        self.scopes
            .get(&(unit.to_string(), file.to_string()))
            .cloned()
    }

    fn write_scope(&self, unit: &str, _file: &str, value: &str) -> std::io::Result<()> {
        if self.scope_directory(unit).is_none() {
            return Err(std::io::Error::other(format!("no scope named {unit}")));
        }
        let _ = value;
        Ok(())
    }

    fn effective_uid(&self) -> u32 {
        1000
    }

    fn namespace_members(&self, pid: u32) -> Option<Vec<u32>> {
        self.processes.get(&pid).map(|_| vec![pid])
    }

    fn user_scope(&self) -> bool {
        self.user_scope
    }

    fn namespace_members_of(&self, inode: u64) -> Vec<u32> {
        let mut members: Vec<u32> = self
            .namespaces
            .iter()
            .filter(|(pid, namespaces)| {
                namespaces.pid == inode && self.processes.contains_key(pid)
            })
            .map(|(pid, _)| *pid)
            .collect();
        members.sort_unstable();
        members
    }
}

#[cfg(test)]
mod tests {
    use super::{Host, Namespace, NamespaceInodes, ProcessState, SystemHost};

    #[test]
    fn the_namespace_table_is_the_record_order() {
        // The payload keeps the inodes as an array, so the enum's numbering is
        // the record format. Nothing else may disagree with it.
        for (position, namespace) in Namespace::ALL.iter().enumerate() {
            assert_eq!(namespace.index(), position, "{} is out of order", namespace.name());
        }
        let inodes = NamespaceInodes {
            user: 1,
            ipc: 2,
            net: 3,
            pid: 4,
            mnt: 5,
            uts: 6,
            cgroup: 7,
        };
        assert_eq!(
            inodes.entries().map(|(name, _)| name),
            Namespace::ALL.map(Namespace::name)
        );
        assert_eq!(<[u64; 7]>::from(&inodes), [1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(NamespaceInodes::from([1, 2, 3, 4, 5, 6, 7]), inodes);
    }

    #[test]
    fn the_system_host_reads_this_process() {
        use std::os::unix::fs::MetadataExt;

        let host = SystemHost::new();
        let pid = std::process::id();
        let info = host.process(pid).expect("this process");
        let expected = std::fs::metadata("/proc/self").expect("self").uid();
        assert_eq!(info.uid, expected);
        assert_ne!(info.state, ProcessState::Zombie);
        assert!(info.start_time > 0);
    }

    #[test]
    fn the_system_host_finds_a_namespace_by_inode() {
        let host = SystemHost::new();
        let pid = std::process::id();
        let namespaces = host.namespaces(pid).expect("namespaces");
        let members = host.namespace_members_of(namespaces.pid);
        assert!(
            members.contains(&pid),
            "this process should be a member: {members:?}"
        );
    }

    #[test]
    fn the_system_host_reads_the_namespaces_and_root() {
        let host = SystemHost::new();
        let pid = std::process::id();
        let namespaces = host.namespaces(pid).expect("namespaces");
        for (name, inode) in namespaces.entries() {
            assert!(inode > 0, "{name} has no inode");
        }
        assert!(host.root_identity(pid).expect("root").inode > 0);
    }

    #[test]
    fn the_system_host_reads_the_mount_table_and_environment() {
        let host = SystemHost::new();
        let pid = std::process::id();
        let mounts = host.mounts(pid).expect("mounts");
        assert!(
            mounts
                .iter()
                .any(|mount| mount.path == std::path::Path::new("/proc"))
        );
        let environment = host.environment(pid).expect("environment");
        assert!(environment.iter().any(|(name, _)| name == "PATH"));
    }

    #[test]
    fn an_absent_process_has_no_information() {
        let host = SystemHost::new();
        assert!(host.process(u32::MAX).is_none());
        assert!(host.namespaces(u32::MAX).is_none());
        assert!(host.environment(u32::MAX).is_none());
    }
}
