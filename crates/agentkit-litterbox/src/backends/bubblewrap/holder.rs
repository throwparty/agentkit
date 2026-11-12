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
use std::os::fd::{AsRawFd, FromRawFd};
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

/// The utility that blocks for the life of the sandbox.
const SLEEP: &str = "sleep";

/// The mode bits that make the loader ignore a loader trace.
const SET_ID: u32 = 0o6000;

/// The command that keeps a sandbox alive and the paths it needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// The shell that runs the holder command.
    pub shell: PathBuf,
    /// The arguments the shell takes before the script.
    pub args: Vec<String>,
    /// The paths to bind: the shell, the utility and the interpreter and
    /// libraries the host's loader resolves for both.
    pub binds: Vec<PathBuf>,
    /// The directory list the shell searches for the utility, which is not the
    /// sandbox's `PATH`.
    pub path: String,
    /// The links the sandbox needs to look like a POSIX system: the mechanism's
    /// shell at `/bin/sh` and, when the host has one, `env` at `/usr/bin/env`.
    pub symlinks: Vec<(PathBuf, PathBuf)>,
}

impl Command {
    /// The shell arguments that hold the sandbox.
    ///
    /// The shell looks the utility up by name, which is what a multicall binary
    /// needs to pick its behaviour, and the assignment keeps that search out of
    /// the sandbox's own environment.
    pub fn arguments(&self) -> Vec<String> {
        let mut arguments = self.args.clone();
        arguments.push(format!("PATH={} exec {SLEEP} infinity", self.path));
        arguments
    }

    /// The program and leading arguments that run a shell script.
    ///
    /// This is the argv a sandbox mounts, so a caller that runs a script can
    /// append it rather than assuming `sh -c` resolves inside the sandbox.
    pub fn argv(&self) -> Vec<String> {
        let mut argv = Vec::with_capacity(self.args.len() + 1);
        argv.push(self.shell.to_string_lossy().to_string());
        argv.extend(self.args.iter().cloned());
        argv
    }

    /// The command that runs a shell script in the sandbox.
    pub fn run(&self, script: &str) -> Vec<String> {
        let mut command = self.argv();
        command.push(script.to_string());
        command
    }
}

/// Resolve the holder command and validate the binaries it runs.
///
/// The shell is a program and the arguments it takes before the script, which
/// is `["sh", "-c"]` unless the configuration says otherwise.
pub fn command(host: &dyn Host, shell: &[String]) -> Result<Command, Error> {
    let argv = shell_argv(host, shell)?;
    let sleep = trusted(host, SLEEP)?;
    let mut binaries = vec![PathBuf::from(&argv[0]), sleep];
    let mut symlinks = vec![(PathBuf::from(&argv[0]), PathBuf::from("/bin/sh"))];
    // `/usr/bin/env` is what a `#!/usr/bin/env <tool>` shebang runs; a
    // devShell's scripts use it even though the sandbox's own PATH may not
    // name it.
    if let Some(env) = optional(host, "env") {
        symlinks.push((env.clone(), PathBuf::from("/usr/bin/env")));
        binaries.push(env);
    }
    let mut binds = Vec::new();
    let mut directories: Vec<String> = Vec::new();
    for path in binaries {
        if !binds.contains(&path) {
            binds.push(path.clone());
        }
        for dependency in runtime(&path, host.ownership(&path).map_or(0, |(_, mode)| mode)) {
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
        shell: PathBuf::from(&argv[0]),
        args: argv[1..].to_vec(),
        binds,
        path: directories.join(":"),
        symlinks,
    })
}

/// Resolve a binary the sandbox benefits from but does not require.
fn optional(host: &dyn Host, name: &str) -> Option<PathBuf> {
    trusted(host, name).ok()
}

/// The shell a sandbox evaluates a script with: the resolved program and the
/// arguments that precede the script.
///
/// A caller that has a script asks for this instead of assuming `sh` is on the
/// sandbox's `PATH`. The program is the one the view binds, so the sandbox can
/// always run it.
pub fn shell_argv(host: &dyn Host, shell: &[String]) -> Result<Vec<String>, Error> {
    let (program, args) = shell.split_first().ok_or_else(|| Error::Misconfigured {
        message: "backends.bubblewrap.shell is empty; it needs at least the program to run"
            .to_string(),
    })?;
    let program = trusted(host, program)?;
    let mut argv = vec![program.to_string_lossy().to_string()];
    argv.extend(args.iter().cloned());
    Ok(argv)
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
pub fn check(host: &dyn Host, shell: &[String]) -> crate::sandbox::report::Check {
    use crate::sandbox::report::{Check, State};
    let primitive = "the holder command".to_string();
    match command(host, shell) {
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
    /// When the sandbox's pid 1 started, if it was known. Teardown refuses to
    /// signal a pid it cannot confirm this way.
    pub sandbox_start: Option<u64>,
    /// The namespaces bwrap reported.
    pub namespaces: NamespaceInodes,
    /// The scope unit holding the sandbox, when there is one.
    pub unit: Option<String>,
}

impl Holder {
    /// Rebuild the holder a record describes.
    ///
    /// This is the one place the payload's field list becomes a `Holder`, so
    /// the identity teardown checks cannot drift from the identity the record
    /// keeps. A start time of zero is the sentinel for one that could not be
    /// read, and is recorded as unknown rather than as a value to match.
    pub fn from_payload(data: &super::payload::Payload) -> Self {
        Self {
            launcher_pid: data.launcher_pid,
            sandbox_pid: data.sandbox_pid,
            sandbox_start: (data.sandbox_start != 0).then_some(data.sandbox_start),
            namespaces: NamespaceInodes::from(data.namespaces),
            unit: data.scope.clone(),
        }
    }
}

/// Parse bwrap's status document.
pub fn parse_status(text: &str) -> Option<Status> {
    let text = text.trim();
    let text = text.strip_prefix('{').unwrap_or(text);
    let text = text.strip_suffix('}').unwrap_or(text);
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
    for field in text.split(',') {
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
    // bwrap reports only the namespaces it unshared. The user namespace is
    // never in the document, and a host-network sandbox has no net namespace
    // to report; both are read from the process afterwards.
    if [1, 3, 4, 5, 6].iter().any(|index| inodes[*index] == 0) {
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
    /// The scope launcher and its unit, when a scope is used.
    pub scope: Option<(&'a Path, &'a str)>,
    /// Where bwrap writes its status document.
    pub host: &'a dyn Host,
    pub status_path: &'a Path,
    /// Where bwrap's diagnostics go.
    pub stderr_path: &'a Path,
    /// The mount descriptors the mechanism is handed, which are close-on-exec
    /// everywhere else.
    pub inherit: &'a [i32],
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
    let mut command = if let Some((launcher, unit)) = start.scope {
        argv = scoped_argv(launcher, unit, &argv);
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
    if start.scope.is_some() {
        // The scope launcher reaches the user manager over the caller's bus,
        // and bwrap unsets these before the command runs, so the sandbox never
        // sees them.
        for name in ["XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"] {
            if let Ok(value) = std::env::var(name) {
                command.env(name, value);
            }
        }
    }
    let status_fd = status.as_raw_fd();
    let inherit = start.inherit.to_vec();
    // SAFETY: dup2, fcntl and close are async-signal-safe and every descriptor
    // is open for the lifetime of the spawn.
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(move || {
            if libc::dup2(status_fd, 3) < 0 {
                return Err(io::Error::last_os_error());
            }
            // `dup2` with the same descriptor does nothing, so the close-on-exec
            // flag has to be cleared explicitly or the status file never
            // reaches bwrap.
            if libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            // The mount descriptors are close-on-exec everywhere else; this is
            // the one launch that is allowed to carry them.
            for descriptor in &inherit {
                if libc::fcntl(*descriptor, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }

    let mut child = command.spawn().map_err(|error| Error::Unavailable {
        backend: BACKEND.to_string(),
        reason: format!("{} could not be started: {error}", argv[0]),
        remedy: "install bubblewrap or correct the backend's binary path".to_string(),
    })?;
    let launcher_pid = child.id();

    let deadline = Instant::now() + STATUS_TIMEOUT;
    loop {
        let text = std::fs::read_to_string(start.status_path).unwrap_or_default();
        if let Some(status) = parse_status(&text) {
            // bwrap's document names six namespaces: the sandbox's own user
            // namespace is not one of them, so it is read from the process.
            let namespaces = start
                .host
                .namespaces(status.sandbox_pid)
                .unwrap_or(status.namespaces);
            let sandbox_start = start
                .host
                .process(status.sandbox_pid)
                .map(|process| process.start_time);
            // The launcher outlives this call, so nothing here can wait for it.
            // Dropping a `Child` does not reap it, so a thread waits for it
            // instead; otherwise every deleted sandbox leaves the server
            // holding a zombie.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(Holder {
                launcher_pid,
                sandbox_pid: status.sandbox_pid,
                sandbox_start,
                namespaces,
                unit: start.scope.map(|(_, unit)| unit.to_string()),
            });
        }
        if Instant::now() >= deadline {
            let diagnostic = diagnostic(start.stderr_path);
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::failed(format!(
                "the sandbox did not report itself within {}s{diagnostic}",
                STATUS_TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Wrap bwrap's argv in a user scope, and keep the bus variables out of the
/// sandbox.
fn scoped_argv(launcher: &Path, unit: &str, argv: &[String]) -> Vec<String> {
    let mut inner = argv.to_vec();
    if let Some(separator) = inner.iter().position(|argument| argument == "--") {
        inner.splice(
            separator..separator,
            [
                "--unsetenv".to_string(),
                "XDG_RUNTIME_DIR".to_string(),
                "--unsetenv".to_string(),
                "DBUS_SESSION_BUS_ADDRESS".to_string(),
            ],
        );
    }
    let mut scoped = vec![
        launcher.to_string_lossy().to_string(),
        "--user".to_string(),
        "--scope".to_string(),
        "--quiet".to_string(),
        "--unit".to_string(),
        format!("{unit}.scope"),
        "--".to_string(),
    ];
    scoped.extend(inner);
    scoped
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
///
/// The recorded start time is what distinguishes the sandbox from whatever
/// holds its pid now, so a stale record cannot kill an unrelated process.
pub fn teardown(host: &dyn Host, holder: &Holder, timeout: Duration) -> Result<(), Error> {
    if holder.sandbox_pid != 0 {
        // A stored pid of zero would name the caller's process group, so it is
        // refused here, and the recorded start time refuses a pid that has
        // been reused.
        if let Some(start) = holder.sandbox_start {
            signal(host, holder.sandbox_pid, start, libc::SIGKILL)?;
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

    // Pid 1 of the namespace may already be gone while its children live on,
    // reparented and still holding the workspace open. Address them by the
    // namespace inode the record kept for exactly this.
    if holder.namespaces.pid != 0 {
        for member in host.namespace_members_of(holder.namespaces.pid) {
            if member == holder.sandbox_pid || host.process(member).is_none() {
                continue;
            }
            // SAFETY: signalling a member of the dead sandbox is the point.
            let result = unsafe { libc::kill(member as i32, libc::SIGKILL) };
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(Error::failed(format!(
                        "the sandbox left pid {member} behind: {error}"
                    )));
                }
            }
        }
    }

    Ok(())
}

/// Signal a process, refusing a pid that has been reused.
///
/// A pidfd pins the process between the check and the signal, so the identity
/// that was validated is the one that receives it.
fn signal(host: &dyn Host, pid: u32, start: u64, number: i32) -> Result<(), Error> {
    // SAFETY: pidfd_open has no preconditions and reports a stale pid as an
    // error rather than acting on it.
    let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if descriptor >= 0 {
        // SAFETY: the syscall returned a descriptor this value now owns.
        let descriptor = unsafe { std::os::fd::OwnedFd::from_raw_fd(descriptor as i32) };
        if !identity_matches(host, pid, start) {
            return Ok(());
        }
        // SAFETY: the pidfd names the process that was just validated.
        let outcome = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                descriptor.as_raw_fd(),
                number,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if outcome < 0 {
            return Err(Error::failed(format!(
                "the sandbox could not be signalled: {}",
                io::Error::last_os_error()
            )));
        }
        return Ok(());
    }
    // The kernel may not offer pidfds; checking again before the signal leaves
    // only the window between the check and the call.
    if !identity_matches(host, pid, start) {
        return Ok(());
    }
    // SAFETY: kill has no preconditions; the pid was just validated.
    let outcome = unsafe { libc::kill(pid as i32, number) };
    if outcome < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(Error::failed(format!(
                "the sandbox could not be killed: {error}"
            )));
        }
    }
    Ok(())
}

fn identity_matches(host: &dyn Host, pid: u32, start: u64) -> bool {
    host.process(pid)
        .is_some_and(|process| process.start_time == start)
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

    const STATUS: &str = "{\"child-pid\": 4242, \"cgroup-namespace\": 7, \"ipc-namespace\": 2, \"mnt-namespace\": 5, \"net-namespace\": 3, \"pid-namespace\": 4, \"uts-namespace\": 6}\n";

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
        let command = command(&holder_host(), &default_shell()).expect("command");
        assert_eq!(command.shell, PathBuf::from("/usr/bin/sh"));
        assert_eq!(command.args, vec!["-c".to_string()]);
        assert!(command.binds.contains(&PathBuf::from("/usr/bin/sh")));
        assert!(command.binds.contains(&PathBuf::from("/usr/bin/sleep")));
        assert_eq!(
            command.arguments(),
            vec![
                "-c".to_string(),
                "PATH=/usr/bin exec sleep infinity".to_string()
            ]
        );
        assert_eq!(
            command.run("echo hi"),
            vec![
                "/usr/bin/sh".to_string(),
                "-c".to_string(),
                "echo hi".to_string()
            ]
        );
        assert!(
            command
                .symlinks
                .contains(&(PathBuf::from("/usr/bin/sh"), PathBuf::from("/bin/sh"))),
            "a shebang needs /bin/sh"
        );
    }

    #[test]
    fn env_is_linked_when_the_host_has_one() {
        let mut host = holder_host();
        host.binaries
            .insert("env".to_string(), PathBuf::from("/opt/bin/env"));
        host.ownership
            .insert(PathBuf::from("/opt/bin/env"), (0, 0o755));
        let command = command(&host, &default_shell()).expect("command");
        assert!(
            command
                .symlinks
                .contains(&(PathBuf::from("/opt/bin/env"), PathBuf::from("/usr/bin/env"))),
            "#!/usr/bin/env needs the link"
        );
        assert!(command.binds.contains(&PathBuf::from("/opt/bin/env")));
    }

    fn default_shell() -> Vec<String> {
        crate::backends::bubblewrap::backend::DEFAULT_SHELL
            .map(str::to_string)
            .to_vec()
    }

    #[test]
    fn a_configured_shell_replaces_the_default() {
        let shell = vec!["sh".to_string(), "-ec".to_string()];
        let command = command(&holder_host(), &shell).expect("command");
        assert_eq!(command.args, vec!["-ec".to_string()]);
        assert_eq!(
            command.run("false"),
            vec![
                "/usr/bin/sh".to_string(),
                "-ec".to_string(),
                "false".to_string()
            ]
        );
    }

    #[test]
    fn an_empty_shell_is_a_configuration_error() {
        let error = command(&holder_host(), &[]).expect_err("refused");
        assert!(error.to_string().contains("shell"), "{error}");
    }

    #[test]
    fn a_holder_binary_outside_the_host_is_unavailable() {
        let mut host = holder_host();
        host.binaries.remove("sleep");
        let error = command(&host, &default_shell()).expect_err("refused");
        assert!(error.to_string().contains("sleep"), "{error}");
    }

    #[test]
    fn a_user_owned_holder_binary_is_refused() {
        let mut host = holder_host();
        host.ownership.insert(PathBuf::from("/usr/bin/sh"), (1000, 0o755));
        let error = command(&host, &default_shell()).expect_err("refused");
        assert!(error.to_string().contains("root-owned"), "{error}");
    }

    #[test]
    fn the_status_document_is_parsed() {
        let status = parse_status(STATUS).expect("parsed");
        assert_eq!(status.sandbox_pid, 4242);
        assert_eq!(
            status.namespaces.entries(),
            [
                ("user", 0),
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
    fn a_document_without_a_network_namespace_is_parsed() {
        let status = parse_status(
            "{\"child-pid\": 4242, \"cgroup-namespace\": 7, \"ipc-namespace\": 2, \
             \"mnt-namespace\": 5, \"pid-namespace\": 4, \"uts-namespace\": 6}\n",
        )
        .expect("parsed");
        assert_eq!(status.sandbox_pid, 4242);
        assert_eq!(status.namespaces.net, 0);
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
        fn effective_uid(&self) -> u32 {
            self.inner.effective_uid()
        }
        fn namespace_members(&self, pid: u32) -> Option<Vec<u32>> {
            self.inner.namespace_members(pid)
        }
        fn namespace_members_of(&self, inode: u64) -> Vec<u32> {
            self.inner.namespace_members_of(inode)
        }
        fn user_scope(&self) -> bool {
            self.inner.user_scope()
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
        // A pid the kernel cannot allocate, so the test never signals a real
        // process: pid 11 is a kernel worker on some hosts and kill returns
        // EPERM there.
        let holder = Holder {
            launcher_pid: 2_000_000_000,
            sandbox_pid: 2_000_000_000,
            sandbox_start: None,
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

    #[test]
    fn a_record_without_a_pid_never_signals_the_process_group() {
        let host = FakeHost::default();
        let holder = Holder {
            launcher_pid: 0,
            sandbox_pid: 0,
            sandbox_start: None,
            namespaces: NamespaceInodes {
                user: 0,
                ipc: 0,
                net: 0,
                pid: 0,
                mnt: 0,
                uts: 0,
                cgroup: 0,
            },
            unit: None,
        };
        teardown(&host, &holder, Duration::from_millis(50)).expect("nothing to tear down");
    }
}
