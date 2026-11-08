//! The holder command, and starting the sandbox around it.
//!
//! The command the sandbox is held open by is the mechanism's own requirement:
//! bwrap has to run something for as long as the sandbox exists, and that
//! something is a shell that execs one blocking utility. What a workload can
//! run is whatever its provisioning steps contribute.
//!
//! bwrap writes its status document before the view is built, so create waits
//! for the recorded mounts as well. The sandbox is detached from the caller:
//! no standard stream is inherited, no session is held and no parent-death tie
//! is emitted, so it outlives the tool the way a branch does.

use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::BACKEND;
use super::host::{Host, MountEntry, NamespaceInodes};
use super::view::Mount;
use crate::sandbox::contract::Error;
use crate::sandbox::environment::Environment;

/// How long bwrap has to report its status document.
const STATUS_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the view has to finish building.
pub const READY_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The shell the holder command is evaluated with.
const SHELL: &str = "sh";

/// The utility that blocks for the life of the sandbox.
const SLEEP: &str = "sleep";

/// The mode bits that make the loader ignore a loader trace.
const SET_ID: u32 = 0o6000;

/// The command that keeps a sandbox alive and the paths it needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// The shell that runs the holder command.
    pub shell: PathBuf,
    /// The paths to bind: the shell, the utility and the interpreter and
    /// libraries the host's loader resolves for both.
    pub binds: Vec<PathBuf>,
    /// The directory list the shell searches for the utility, which is not the
    /// sandbox's `PATH`.
    pub path: String,
}

impl Command {
    /// The shell arguments that hold the sandbox.
    ///
    /// The shell looks the utility up by name, which is what a multicall binary
    /// needs to pick its behaviour, and the assignment keeps that search out of
    /// the sandbox's own environment.
    pub fn arguments(&self) -> [String; 2] {
        [
            "-c".to_string(),
            format!("PATH={} exec {SLEEP} infinity", self.path),
        ]
    }
}

/// Resolve the holder command and validate the binaries it runs.
pub fn command(host: &dyn Host) -> Result<Command, Error> {
    let shell = trusted(host, SHELL)?;
    let sleep = trusted(host, SLEEP)?;
    let mut binds = Vec::new();
    let mut directories: Vec<String> = Vec::new();
    for path in [&shell, &sleep] {
        if !binds.contains(path) {
            binds.push(path.clone());
        }
        for dependency in runtime(path, host.ownership(path).map_or(0, |(_, mode)| mode)) {
            if !binds.contains(&dependency) {
                binds.push(dependency);
            }
        }
        if let Some(directory) = path.parent() {
            // The utility is reached by name, and a multicall binary is a link
            // in that directory, so the directory has to be in the view.
            if !binds.iter().any(|known| known == directory) {
                binds.push(directory.to_path_buf());
            }
            let directory = directory.to_string_lossy().to_string();
            if !directories.contains(&directory) {
                directories.push(directory);
            }
        }
    }
    Ok(Command {
        shell,
        binds,
        path: directories.join(":"),
    })
}

/// Resolve a binary and check that the sandbox can trust it.
fn trusted(host: &dyn Host, name: &str) -> Result<PathBuf, Error> {
    let path = host.resolve(name).ok_or_else(|| Error::Unavailable {
        backend: BACKEND.to_string(),
        reason: format!("{name} does not resolve on the host"),
        remedy: format!("install {name} or add the directory holding it to PATH"),
    })?;
    let (uid, mode) = host.ownership(&path).ok_or_else(|| Error::Unavailable {
        backend: BACKEND.to_string(),
        reason: format!("{} cannot be inspected", path.display()),
        remedy: "check that the file is readable".to_string(),
    })?;
    if uid != 0 || mode & 0o022 != 0 {
        return Err(Error::Unavailable {
            backend: BACKEND.to_string(),
            reason: format!(
                "{} is not root-owned and unshared, so the sandbox will not run it",
                path.display()
            ),
            remedy: "install the utility from a package manager that owns its files".to_string(),
        });
    }
    Ok(path)
}

/// Whether the host can hold a sandbox, as a readiness check.
pub fn check(host: &dyn Host) -> crate::sandbox::report::Check {
    use crate::sandbox::report::{Check, State};
    let primitive = "the holder command".to_string();
    match command(host) {
        Ok(_) => Check {
            primitive,
            state: State::Available,
            reason: None,
            remedy: None,
            fallback: None,
        },
        Err(error) => Check {
            primitive,
            state: State::Unavailable,
            reason: Some(error.to_string()),
            remedy: Some("install a shell and coreutils on the host".to_string()),
            fallback: None,
        },
    }
}

/// The absolute paths a binary needs in order to run.
///
/// The loader resolves a binary's interpreter and libraries from the binary
/// itself, so asking it is the only answer that holds for any layout. The trace
/// loads the binary the way `ldd` does and reports what it resolved; a static
/// binary reports nothing. A set-id binary is not traced, because the loader
/// ignores the trace for one and would run it instead.
pub(crate) fn runtime(path: &Path, mode: u32) -> Vec<PathBuf> {
    if mode & SET_ID != 0 {
        return Vec::new();
    }
    let Ok(output) = std::process::Command::new(path)
        .env_clear()
        .env("LD_TRACE_LOADED_OBJECTS", "1")
        .stdin(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    parse_trace(&String::from_utf8_lossy(&output.stdout))
        .into_iter()
        .filter(|dependency| dependency.exists())
        .collect()
}

/// The absolute paths in a loader trace.
fn parse_trace(text: &str) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for line in text.lines() {
        // A library reads `name => /path (0x…)`; the interpreter is the bare
        // `/path (0x…)`. The virtual objects have no path at all.
        let path = match line.split_once(" => ") {
            Some((_, rest)) => rest.split_whitespace().next().unwrap_or_default(),
            None => line.split_whitespace().next().unwrap_or_default(),
        };
        let path = Path::new(path);
        if path.is_absolute() && !paths.iter().any(|known| known == path) {
            paths.push(path.to_path_buf());
        }
    }
    paths
}

/// How long a killed sandbox has to disappear.
const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(30);

/// What bwrap reports about the sandbox it made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// The pid of the sandbox's pid 1.
    pub sandbox_pid: u32,
    /// The seven namespace inodes it holds.
    pub namespaces: NamespaceInodes,
}

/// A sandbox that has started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holder {
    /// The process the tool started, which is bwrap or the scope launcher.
    pub launcher_pid: u32,
    /// The sandbox's pid 1.
    pub sandbox_pid: u32,
    /// The namespaces bwrap reported.
    pub namespaces: NamespaceInodes,
    /// The scope unit holding the sandbox, when there is one.
    pub unit: Option<String>,
}

/// Parse bwrap's status document.
pub fn parse_status(text: &str) -> Option<Status> {
    let mut sandbox_pid = None;
    let mut inodes = [0u64; 7];
    let names = [
        "user-namespace",
        "ipc-namespace",
        "net-namespace",
        "pid-namespace",
        "mnt-namespace",
        "uts-namespace",
        "cgroup-namespace",
    ];
    for field in text
        .trim_matches(|character| character == '{' || character == '}')
        .split(',')
    {
        let (name, value) = field.split_once(':')?;
        let name = name.trim().trim_matches('"');
        let value = value.trim().parse::<u64>().ok()?;
        if name == "child-pid" {
            sandbox_pid = Some(u32::try_from(value).ok()?);
            continue;
        }
        if let Some(index) = names.iter().position(|candidate| *candidate == name) {
            inodes[index] = value;
        }
    }
    let sandbox_pid = sandbox_pid?;
    if inodes.contains(&0) {
        return None;
    }
    Some(Status {
        sandbox_pid,
        namespaces: NamespaceInodes {
            user: inodes[0],
            ipc: inodes[1],
            net: inodes[2],
            pid: inodes[3],
            mnt: inodes[4],
            uts: inodes[5],
            cgroup: inodes[6],
        },
    })
}

/// The command line that starts one sandbox.
pub struct Start<'a> {
    /// The rendered bwrap argv.
    pub argv: &'a [String],
    /// The environment the sandbox receives.
    pub environment: &'a Environment,
    /// The scope unit to start it in, when one is used.
    pub unit: Option<&'a str>,
    /// Where bwrap writes its status document.
    pub status_path: &'a Path,
    /// Where bwrap's diagnostics go.
    pub stderr_path: &'a Path,
}

/// Start the sandbox and return once bwrap has reported it.
///
/// The view is not built yet: `await_ready` is what makes it usable.
pub fn start(start: &Start<'_>) -> Result<Holder, Error> {
    let status = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(start.status_path)
        .map_err(|error| Error::failed(format!("the status file could not be created: {error}")))?;
    let stderr = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(start.stderr_path)
        .map_err(|error| {
            Error::failed(format!("the diagnostic file could not be created: {error}"))
        })?;

    let mut argv = start.argv.to_vec();
    let mut command = if let Some(unit) = start.unit {
        let mut scoped = vec![
            "systemd-run".to_string(),
            "--user".to_string(),
            "--scope".to_string(),
            "--quiet".to_string(),
            "--unit".to_string(),
            format!("{unit}.scope"),
            "--".to_string(),
        ];
        scoped.append(&mut argv);
        argv = scoped;
        let mut command = std::process::Command::new(&argv[0]);
        command.args(&argv[1..]);
        command
    } else {
        let mut command = std::process::Command::new(&argv[0]);
        command.args(&argv[1..]);
        command
    };

    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(stderr));
    command.env_clear();
    command.envs(&start.environment.variables);
    let status_fd = status.as_raw_fd();
    // SAFETY: dup2 is async-signal-safe and the descriptor is open for the
    // lifetime of the spawn.
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(move || {
            if libc::dup2(status_fd, 3) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = command.spawn().map_err(|error| Error::Unavailable {
        backend: BACKEND.to_string(),
        reason: format!("{} could not be started: {error}", argv[0]),
        remedy: "install bubblewrap or correct the backend's binary path".to_string(),
    })?;
    let launcher_pid = child.id();

    let deadline = Instant::now() + STATUS_TIMEOUT;
    loop {
        let text = std::fs::read_to_string(start.status_path).unwrap_or_default();
        if let Some(status) = parse_status(&text) {
            return Ok(Holder {
                launcher_pid,
                sandbox_pid: status.sandbox_pid,
                namespaces: status.namespaces,
                unit: start.unit.map(str::to_string),
            });
        }
        if Instant::now() >= deadline {
            let diagnostic = diagnostic(start.stderr_path);
            return Err(Error::failed(format!(
                "the sandbox did not report itself within {}s{diagnostic}",
                STATUS_TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Wait until every expected mount is present with its recorded access.
pub async fn await_ready(
    host: &dyn Host,
    sandbox_pid: u32,
    expected: &[Mount],
    timeout: Duration,
) -> Result<(), Error> {
    let deadline = Instant::now() + timeout;
    loop {
        if host.process(sandbox_pid).is_none() {
            return Err(Error::failed("the sandbox exited while it was starting"));
        }
        if let Some(table) = host.mounts(sandbox_pid)
            && view_is_ready(&table, expected)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Error::failed(format!(
                "the sandbox view did not finish within {}s",
                timeout.as_secs()
            )));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Whether the table holds every expected mount with its recorded access.
fn view_is_ready(table: &[MountEntry], expected: &[Mount]) -> bool {
    expected.iter().all(|mount| {
        table
            .iter()
            .any(|entry| entry.path == mount.path && entry.read_only == mount.read_only())
    })
}

/// Kill the sandbox and wait for it to disappear.
pub fn teardown(host: &dyn Host, holder: &Holder, timeout: Duration) -> Result<(), Error> {
    // SAFETY: kill has no preconditions; a stale pid is refused by validation
    // before this is called.
    let result = unsafe { libc::kill(holder.sandbox_pid as i32, libc::SIGKILL) };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(Error::failed(format!(
                "the sandbox could not be killed: {error}"
            )));
        }
    }
    if let Some(unit) = &holder.unit {
        let _ = host.write_scope(unit, "cgroup.kill", "1");
    }
    let deadline = Instant::now() + timeout;
    while host.process(holder.sandbox_pid).is_some() {
        if Instant::now() >= deadline {
            return Err(Error::failed(format!(
                "sandbox '{}' did not stop within {}s",
                holder.sandbox_pid,
                timeout.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

fn diagnostic(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let bounded: String = trimmed.chars().take(2048).collect();
    format!(": {bounded}")
}

impl Mount {
    /// Whether the mount is read-only.
    pub fn read_only(&self) -> bool {
        self.access == super::view::Access::ReadOnly
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::{Holder, await_ready, command, parse_status, parse_trace, teardown, view_is_ready};
    use crate::backends::bubblewrap::host::{
        FakeHost, Host, MountEntry, NamespaceInodes, ProcessInfo, ProcessState,
    };
    use crate::backends::bubblewrap::view::{Access, Mount};

    const STATUS: &str = r#"{ "child-pid": 4242, "cgroup-namespace": 7, "ipc-namespace": 2, "mnt-namespace": 5, "net-namespace": 3, "pid-namespace": 4, "user-namespace": 1, "uts-namespace": 6 }"#;

    #[test]
    fn a_loader_trace_names_the_libraries_and_the_interpreter() {
        let trace = "\tlinux-vdso.so.1 (0x00007ffd)\n\
                     \tlibreadline.so.8 => /usr/lib/libreadline.so.8 (0x00007f)\n\
                     \tlibc.so.6 => /usr/lib/libc.so.6 (0x00007f)\n\
                     \t/usr/lib/ld-linux-x86-64.so.2 (0x00007f)\n";
        assert_eq!(
            parse_trace(trace),
            vec![
                PathBuf::from("/usr/lib/libreadline.so.8"),
                PathBuf::from("/usr/lib/libc.so.6"),
                PathBuf::from("/usr/lib/ld-linux-x86-64.so.2"),
            ]
        );
        assert!(parse_trace("not a dynamic executable\n").is_empty());
    }

    fn holder_host() -> FakeHost {
        let mut host = FakeHost::default();
        for name in ["sh", "sleep"] {
            let path = PathBuf::from("/usr/bin").join(name);
            host.binaries.insert(name.to_string(), path.clone());
            host.ownership.insert(path, (0, 0o755));
        }
        host
    }

    #[test]
    fn the_holder_command_binds_the_shell_and_the_utility() {
        let command = command(&holder_host()).expect("command");
        assert_eq!(command.shell, PathBuf::from("/usr/bin/sh"));
        assert!(command.binds.contains(&PathBuf::from("/usr/bin/sh")));
        assert!(command.binds.contains(&PathBuf::from("/usr/bin/sleep")));
        assert_eq!(
            command.arguments(),
            [
                "-c".to_string(),
                "PATH=/usr/bin exec sleep infinity".to_string()
            ]
        );
    }

    #[test]
    fn a_holder_binary_outside_the_host_is_unavailable() {
        let mut host = holder_host();
        host.binaries.remove("sleep");
        let error = command(&host).expect_err("refused");
        assert!(error.to_string().contains("sleep"), "{error}");
    }

    #[test]
    fn a_user_owned_holder_binary_is_refused() {
        let mut host = holder_host();
        host.ownership.insert(PathBuf::from("/usr/bin/sh"), (1000, 0o755));
        let error = command(&host).expect_err("refused");
        assert!(error.to_string().contains("root-owned"), "{error}");
    }

    #[test]
    fn the_status_document_is_parsed() {
        let status = parse_status(STATUS).expect("parsed");
        assert_eq!(status.sandbox_pid, 4242);
        assert_eq!(
            status.namespaces.entries(),
            [
                ("user", 1),
                ("ipc", 2),
                ("net", 3),
                ("pid", 4),
                ("mnt", 5),
                ("uts", 6),
                ("cgroup", 7),
            ]
        );
    }

    #[test]
    fn a_truncated_document_is_not_a_status() {
        assert!(parse_status(r#"{ "child-pid": 4242 }"#).is_none());
        assert!(parse_status("").is_none());
        assert!(parse_status(r#"{ "child-pid": "x" }"#).is_none());
    }

    #[test]
    fn the_view_is_ready_only_with_every_mount_and_access() {
        let expected = vec![
            Mount {
                path: PathBuf::from("/nix/store/abc"),
                access: Access::ReadOnly,
            },
            Mount {
                path: PathBuf::from("/nix/store/def"),
                access: Access::ReadOnly,
            },
        ];
        let entry = |path: &str, read_only: bool| MountEntry {
            path: PathBuf::from(path),
            read_only,
            fstype: "ext4".to_string(),
        };
        assert!(!view_is_ready(&[entry("/nix/store/abc", true)], &expected));
        assert!(view_is_ready(
            &[entry("/nix/store/abc", true), entry("/nix/store/def", true)],
            &expected
        ));
        assert!(!view_is_ready(
            &[
                entry("/nix/store/abc", true),
                entry("/nix/store/def", false)
            ],
            &expected
        ));
    }

    struct CountingHost {
        inner: FakeHost,
        calls: AtomicUsize,
        become_ready_at: usize,
    }

    impl Host for CountingHost {
        fn process(&self, pid: u32) -> Option<ProcessInfo> {
            self.inner.process(pid)
        }
        fn namespaces(&self, pid: u32) -> Option<NamespaceInodes> {
            self.inner.namespaces(pid)
        }
        fn root_identity(
            &self,
            pid: u32,
        ) -> Option<crate::backends::bubblewrap::host::RootIdentity> {
            self.inner.root_identity(pid)
        }
        fn mounts(&self, pid: u32) -> Option<Vec<MountEntry>> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call < self.become_ready_at {
                return Some(Vec::new());
            }
            self.inner.mounts(pid)
        }
        fn environment(&self, pid: u32) -> Option<Vec<(String, String)>> {
            self.inner.environment(pid)
        }
        fn resolve(&self, name: &str) -> Option<PathBuf> {
            self.inner.resolve(name)
        }
        fn ownership(&self, path: &std::path::Path) -> Option<(u32, u32)> {
            self.inner.ownership(path)
        }
        fn scope_directory(&self, unit: &str) -> Option<PathBuf> {
            self.inner.scope_directory(unit)
        }
        fn read_scope(&self, unit: &str, file: &str) -> Option<String> {
            self.inner.read_scope(unit, file)
        }
        fn write_scope(&self, unit: &str, file: &str, value: &str) -> std::io::Result<()> {
            self.inner.write_scope(unit, file, value)
        }
    }

    #[tokio::test]
    async fn the_view_is_waited_for() {
        let mut inner = FakeHost::default();
        inner.processes.insert(
            11,
            ProcessInfo {
                state: ProcessState::Waiting,
                start_time: 1,
                uid: 1000,
            },
        );
        inner.mounts.insert(
            11,
            vec![MountEntry {
                path: PathBuf::from("/nix/store/abc"),
                read_only: true,
                fstype: "ext4".to_string(),
            }],
        );
        let host = CountingHost {
            inner,
            calls: AtomicUsize::new(0),
            become_ready_at: 3,
        };
        let expected = vec![Mount {
            path: PathBuf::from("/nix/store/abc"),
            access: Access::ReadOnly,
        }];
        await_ready(&host, 11, &expected, Duration::from_millis(200))
            .await
            .expect("ready");
        assert!(host.calls.load(Ordering::SeqCst) >= 3);
    }

    #[tokio::test]
    async fn a_view_that_never_builds_is_a_failure() {
        let host = FakeHost::default();
        let expected = vec![Mount {
            path: PathBuf::from("/nix/store/abc"),
            access: Access::ReadOnly,
        }];
        let error = await_ready(&host, 11, &expected, Duration::from_millis(30))
            .await
            .expect_err("refused");
        assert!(error.to_string().contains("exited"), "{error}");
    }

    #[test]
    fn a_sandbox_that_is_already_gone_tears_down_cleanly() {
        let host = FakeHost::default();
        let holder = Holder {
            launcher_pid: 10,
            sandbox_pid: 11,
            namespaces: NamespaceInodes {
                user: 1,
                ipc: 2,
                net: 3,
                pid: 4,
                mnt: 5,
                uts: 6,
                cgroup: 7,
            },
            unit: None,
        };
        teardown(&host, &holder, Duration::from_millis(50)).expect("torn down");
    }
}
