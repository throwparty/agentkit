//! Running one command inside a live sandbox.
//!
//! This is done in this process, not by `nsenter`: every descriptor is opened
//! before anything is joined, the user namespace that owns the others is
//! entered first, and the command is a grandchild so it really is a member of
//! the sandbox's pid namespace.

use std::ffi::CString;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::Mutex;

use super::BACKEND;
use super::host::Host;
use super::payload::BubblewrapData;
use super::view::WORKSPACE;
use crate::sandbox::contract::{Error, Execution};

/// The ioctl that returns a namespace's owning user namespace.
const NS_GET_USERNS: libc::c_ulong = 0xB701;

/// A join step, reported over the status pipe when it fails.
///
/// The code travels beside the command's exit status rather than through it,
/// because every exit status a program can choose is one the command may
/// legitimately return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Joining the owning user namespace.
    OwnUser,
    /// Joining the ipc namespace.
    Ipc,
    /// Joining the network namespace.
    Net,
    /// Joining the pid namespace.
    Pid,
    /// Joining the mount namespace.
    Mnt,
    /// Joining the uts namespace.
    Uts,
    /// Joining the cgroup namespace.
    Cgroup,
    /// Joining the sandbox's own user namespace.
    SandboxUser,
    /// Attaching the command to the sandbox's cgroup.
    Attach,
    /// Forking the command.
    Fork,
    /// Chrooting to the pinned root.
    Chroot,
    /// Changing to the workspace.
    Chdir,
    /// Redirecting the standard streams.
    Streams,
    /// Setting no-new-privileges.
    NoNewPrivileges,
    /// Setting the uid and gid.
    Credentials,
    /// Clearing capabilities.
    Capabilities,
    /// Verifying the joined namespaces.
    Verify,
    /// Executing the command.
    Exec,
}

impl Step {
    /// The byte this step reports over the status pipe.
    pub fn code(self) -> u8 {
        self as u8
    }

    /// The step a reported code names.
    pub fn from_code(code: u8) -> Option<Self> {
        const STEPS: [Step; 18] = [
            Step::OwnUser,
            Step::Ipc,
            Step::Net,
            Step::Pid,
            Step::Mnt,
            Step::Uts,
            Step::Cgroup,
            Step::SandboxUser,
            Step::Attach,
            Step::Fork,
            Step::Chroot,
            Step::Chdir,
            Step::Streams,
            Step::NoNewPrivileges,
            Step::Credentials,
            Step::Capabilities,
            Step::Verify,
            Step::Exec,
        ];
        STEPS.iter().copied().find(|step| step.code() == code)
    }
}

/// Run a command inside the sandbox described by the payload.
///
/// The caller's output is bounded at `cap` bytes per stream; everything past
/// it is drained and dropped so the command never blocks on a full pipe.
pub fn run(
    host: &dyn Host,
    data: &BubblewrapData,
    command: &[String],
    cap: usize,
) -> Result<Execution, Error> {
    let variables = host.environment(data.sandbox_pid).ok_or_else(|| {
        Error::failed("the sandbox environment can no longer be read, so no command can run in it")
    })?;

    let mut descriptors = Vec::new();
    for name in ["user", "ipc", "net", "pid", "mnt", "uts", "cgroup"] {
        let path = format!("/proc/{}/ns/{name}", data.sandbox_pid);
        let file = std::fs::File::open(&path).map_err(|error| {
            Error::failed(format!("the {name} namespace could not be opened: {error}"))
        })?;
        descriptors.push(file);
    }
    let root = std::fs::File::open(format!("/proc/{}/root", data.sandbox_pid))
        .map_err(|error| Error::failed(format!("the sandbox root could not be opened: {error}")))?;
    let stdin = std::fs::File::open("/dev/null").map_err(|error| {
        Error::failed(format!("the null device could not be opened: {error}"))
    })?;
    // The scope is opened before anything is joined: a member of the scope has
    // to be added through a descriptor, not a path that means something else
    // once the cgroup namespace is entered.
    let scope = match data.scope.as_deref() {
        Some(unit) => Some(
            std::fs::File::open(host.scope_directory(unit).ok_or_else(|| {
                Error::failed(format!(
                    "the sandbox scope '{unit}' cannot be found, so a command would run \
                     outside its bounds"
                ))
            })?)
            .map_err(|error| Error::failed(format!("the sandbox scope cannot be opened: {error}")))?,
        ),
        None => None,
    };
    let user = descriptors[0].as_raw_fd();
    // SAFETY: the descriptor is a namespace descriptor and the ioctl only
    // reads it.
    let owner = unsafe { libc::ioctl(user, NS_GET_USERNS, 0) };
    if owner < 0 {
        return Err(Error::failed(format!(
            "the owning user namespace could not be read: {}",
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: the ioctl returned a new descriptor.
    let owner = unsafe { OwnedFd::from_raw_fd(owner) };

    let (stdout_read, stdout_write) = pipe()?;
    let (stderr_read, stderr_write) = pipe()?;
    let (status_read, status_write) = pipe()?;
    let arguments = cstrings(command)?;
    let environment = variables
        .iter()
        .map(|(name, value)| CString::new(format!("{name}={value}")))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| Error::failed("the environment holds a value a command cannot carry"))?;
    let search_path = variables
        .iter()
        .find(|(name, _)| name == "PATH")
        .map(|(_, value)| value.clone());
    let candidates = candidates(&arguments[0], search_path.as_deref())?;
    let expected = data.namespaces;
    // A namespace the sandbox shares with the caller cannot be entered: it is
    // owned by a user namespace the caller has no capability in, and there is
    // nothing to enter anyway. Host networking is the case that matters.
    let mut skip = [false; 7];
    for (index, name) in ["user", "ipc", "net", "pid", "mnt", "uts", "cgroup"]
        .iter()
        .enumerate()
    {
        if let (Ok(sandbox), Ok(current)) = (
            std::fs::read_link(format!("/proc/{}/ns/{name}", data.sandbox_pid)),
            std::fs::read_link(format!("/proc/self/ns/{name}")),
        ) {
            skip[index] = sandbox == current;
        }
    }

    // `fork` in a threaded process is only safe while no other thread holds a
    // lock the child might need. The child touches no lock, but the lock still
    // separates the launch phase from everything else; it is released as soon
    // as the command has been exec'd, so a long-running command holds nothing.
    let guard = JOIN.lock().unwrap_or_else(|poison| poison.into_inner());

    // SAFETY: the child only performs async-signal-safe calls between fork and
    // exec, and exits rather than unwinding on a failure.
    let child = unsafe { libc::fork() };
    if child < 0 {
        return Err(Error::failed(format!(
            "the join could not fork: {}",
            std::io::Error::last_os_error()
        )));
    }

    if child == 0 {
        let context = Child {
            owner: owner.as_raw_fd(),
            descriptors: descriptors.iter().map(|file| file.as_raw_fd()).collect(),
            root: root.as_raw_fd(),
            stdin: stdin.as_raw_fd(),
            stdout: stdout_write.as_raw_fd(),
            stderr: stderr_write.as_raw_fd(),
            status: status_write.as_raw_fd(),
            scope: scope.as_ref().map(AsRawFd::as_raw_fd),
            arguments: &arguments,
            candidates: &candidates,
            environment: &environment,
            expected,
            skip,
        };
        context.run();
    }

    drop(stdout_write);
    drop(stderr_write);
    drop(status_write);
    drop(descriptors);
    drop(root);
    drop(owner);
    drop(stdin);
    drop(scope);

    let stderr_reader = std::thread::spawn(move || read_bounded(stderr_read, cap));
    let stdout_reader = std::thread::spawn(move || read_bounded(stdout_read, cap));
    // The status pipe reports setup failures and closes when the command is
    // exec'd, so this returns once the launch is over rather than when the
    // command finishes.
    let failure = read_step(status_read);
    drop(guard);

    let mut status = 0;
    // SAFETY: the pid is the child this process forked.
    unsafe { libc::waitpid(child, &mut status, 0) };
    let exit_code = exit_code(status);

    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();

    if let Some(step) = failure {
        return Err(Error::failed(format!(
            "{BACKEND}: the join failed at {} (status {exit_code}){suffix}",
            step_name(step),
            suffix = if stderr.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", stderr.trim())
            }
        )));
    }

    Ok(Execution {
        exit_code,
        stdout,
        stderr,
    })
}

/// The paths execve should try, in order, for a program name.
fn candidates(program: &CString, path: Option<&str>) -> Result<Vec<CString>, Error> {
    let name = program.to_string_lossy();
    if name.contains('/') {
        return Ok(vec![program.clone()]);
    }
    let Some(path) = path else {
        return Ok(vec![program.clone()]);
    };
    path.split(':')
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            CString::new(format!("{entry}/{name}"))
                .map_err(|_| Error::failed("the search path holds a NUL byte"))
        })
        .collect()
}

fn step_name(step: Step) -> &'static str {
    match step {
        Step::OwnUser => "entering the owning user namespace",
        Step::Ipc => "entering the ipc namespace",
        Step::Net => "entering the network namespace",
        Step::Pid => "entering the pid namespace",
        Step::Mnt => "entering the mount namespace",
        Step::Uts => "entering the uts namespace",
        Step::Cgroup => "entering the cgroup namespace",
        Step::SandboxUser => "entering the sandbox user namespace",
        Step::Attach => "attaching to the sandbox cgroup",
        Step::Fork => "forking the command",
        Step::Chroot => "chrooting to the sandbox root",
        Step::Chdir => "changing to the workspace",
        Step::Streams => "redirecting the standard streams",
        Step::NoNewPrivileges => "setting no-new-privileges",
        Step::Credentials => "setting the uid and gid",
        Step::Capabilities => "clearing capabilities",
        Step::Verify => "verifying the joined namespaces",
        Step::Exec => "executing the command",
    }
}

/// The state a forked child needs.
struct Child<'a> {
    owner: RawFd,
    descriptors: Vec<RawFd>,
    root: RawFd,
    stdin: RawFd,
    stdout: RawFd,
    stderr: RawFd,
    status: RawFd,
    scope: Option<RawFd>,
    arguments: &'a [CString],
    candidates: &'a [CString],
    environment: &'a [CString],
    expected: [u64; 7],
    skip: [bool; 7],
}

impl Child<'_> {
    /// Join, drop everything and exec. Never returns: it reports the failed
    /// step over the status pipe and exits.
    fn run(&self) -> ! {
        let order = [
            (Step::Ipc, 1, libc::CLONE_NEWIPC),
            (Step::Net, 2, libc::CLONE_NEWNET),
            (Step::Pid, 3, libc::CLONE_NEWPID),
            (Step::Mnt, 4, libc::CLONE_NEWNS),
            (Step::Uts, 5, libc::CLONE_NEWUTS),
            (Step::Cgroup, 6, libc::CLONE_NEWCGROUP),
        ];

        if setns(self.owner, libc::CLONE_NEWUSER).is_err() {
            self.fail(Step::OwnUser);
        }
        for (step, index, flag) in order {
            if self.skip[index] {
                continue;
            }
            if setns(self.descriptors[index], flag).is_err() {
                self.fail(step);
            }
        }
        if !self.skip[0] && setns(self.descriptors[0], libc::CLONE_NEWUSER).is_err() {
            self.fail(Step::SandboxUser);
        }
        // Entering the cgroup namespace does not put this process in the
        // scope's cgroup, so it is added before it can fork anything.
        if let Some(scope) = self.scope
            && !attach(scope)
        {
            self.fail(Step::Attach);
        }

        // SAFETY: the only calls between here and exec are async-signal-safe.
        let grandchild = unsafe { libc::fork() };
        if grandchild < 0 {
            self.fail(Step::Fork);
        }
        if grandchild > 0 {
            // The status pipe has to reach end-of-file when the command is
            // exec'd, which only happens if this process lets go of its copy.
            // SAFETY: the descriptor is this process's copy of the pipe.
            unsafe { libc::close(self.status) };
            let mut status = 0;
            // SAFETY: waiting for the child this process forked.
            unsafe { libc::waitpid(grandchild, &mut status, 0) };
            exit(exit_code(status));
        }

        // The command must never see the tool's own input: in an MCP server
        // that is the request stream.
        if unsafe { libc::dup2(self.stdin, 0) } < 0
            || unsafe { libc::dup2(self.stdout, 1) } < 0
            || unsafe { libc::dup2(self.stderr, 2) } < 0
        {
            self.fail(Step::Streams);
        }

        // SAFETY: fchdir and chroot operate on the descriptor this child owns.
        let dot = cstr(".");
        if unsafe { libc::fchdir(self.root) } < 0 || unsafe { libc::chroot(dot.as_ptr()) } < 0 {
            self.fail(Step::Chroot);
        }
        if unsafe { libc::chdir(cstr(WORKSPACE).as_ptr()) } < 0 {
            self.fail(Step::Chdir);
        }
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } < 0 {
            self.fail(Step::NoNewPrivileges);
        }
        // SAFETY: the ids are the ones the caller already holds.
        if unsafe { libc::setresgid(libc::getgid(), libc::getgid(), libc::getgid()) } < 0
            || unsafe { libc::setresuid(libc::getuid(), libc::getuid(), libc::getuid()) } < 0
        {
            self.fail(Step::Credentials);
        }
        if !self.capabilities_cleared() {
            self.fail(Step::Capabilities);
        }
        if !self.namespaces_match() {
            self.fail(Step::Verify);
        }

        let argv: Vec<*const libc::c_char> = self
            .arguments
            .iter()
            .map(|argument| argument.as_ptr())
            .chain([std::ptr::null()])
            .collect();
        let envp: Vec<*const libc::c_char> = self
            .environment
            .iter()
            .map(|entry| entry.as_ptr())
            .chain([std::ptr::null()])
            .collect();
        // Every descriptor the tool still holds is dropped here. A mount that
        // was pinned for the view is read-only in the sandbox, so letting the
        // command recover one of those descriptors would let it write through
        // the read-only bind. The status pipe is the one that survives, and it
        // is close-on-exec.
        close_extra(self.status);
        // execve does not search the PATH, so the candidates are resolved
        // against the sandbox's own PATH before the fork.
        for candidate in self.candidates {
            // SAFETY: the pointers are valid and null-terminated.
            unsafe { libc::execve(candidate.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
        }
        self.fail(Step::Exec);
    }

    fn capabilities_cleared(&self) -> bool {
        // The kernel's capset ABI, defined here because libc keeps it behind a
        // feature this target does not enable.
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Header {
            version: u32,
            pid: i32,
        }
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Data {
            effective: u32,
            permitted: u32,
            inheritable: u32,
        }
        const VERSION_3: u32 = 0x2008_0522;
        let mut header = Header {
            version: VERSION_3,
            pid: 0,
        };
        let data = [Data {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        }; 2];
        // SAFETY: the header and the data array match the kernel's ABI.
        unsafe { libc::syscall(libc::SYS_capset, &mut header, data.as_ptr()) == 0 }
    }

    fn namespaces_match(&self) -> bool {
        let names = ["user", "ipc", "net", "pid", "mnt", "uts", "cgroup"];
        for (index, name) in names.iter().enumerate() {
            let Ok(link) = std::fs::read_link(format!("/proc/self/ns/{name}")) else {
                return false;
            };
            let text = link.to_string_lossy();
            let Some(start) = text.find('[') else {
                return false;
            };
            let Some(end) = text.find(']') else {
                return false;
            };
            let Ok(inode) = text[start + 1..end].parse::<u64>() else {
                return false;
            };
            if inode != self.expected[index] {
                return false;
            }
        }
        true
    }

    /// Report a failed step over the status pipe and exit.
    fn fail(&self, step: Step) -> ! {
        let byte = [step.code()];
        // SAFETY: the descriptor is this process's copy of the status pipe and
        // the buffer is one byte.
        unsafe { libc::write(self.status, byte.as_ptr().cast(), 1) };
        exit(1);
    }
}

/// Add this process to the scope's cgroup through the descriptor.
fn attach(scope: RawFd) -> bool {
    let name = cstr("cgroup.procs");
    // SAFETY: the descriptor is a directory and the name is one component.
    let file = unsafe { libc::openat(scope, name.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC) };
    if file < 0 {
        return false;
    }
    // A process adds itself by writing its own pid, which is zero here.
    // SAFETY: the descriptor is open for writing and the buffer is one byte.
    let written = unsafe { libc::write(file, b"0".as_ptr().cast(), 1) };
    // SAFETY: the descriptor was opened above and is not used again.
    unsafe { libc::close(file) };
    written == 1
}

/// Close every descriptor above the standard streams except `keep`.
///
/// Release builds of the tool hold nothing else open at this point, but a
/// descriptor used for the launch must not survive the exec.
fn close_extra(keep: RawFd) {
    // SAFETY: close_range has no preconditions and cannot fail for a valid
    // range; the fallback only runs when the kernel does not know it.
    let below = if keep > 3 {
        unsafe { libc::syscall(libc::SYS_close_range, 3u32, (keep - 1) as u32, 0u32) }
    } else {
        0
    };
    let above = unsafe { libc::syscall(libc::SYS_close_range, (keep + 1) as u32, u32::MAX, 0u32) };
    if below < 0 || above < 0 {
        let mut limit: libc::rlimit = unsafe { std::mem::zeroed() };
        // SAFETY: getrlimit writes the limit it is asked for.
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) };
        let highest = (limit.rlim_cur as i32).clamp(3, 1 << 16);
        for descriptor in 3..highest {
            if descriptor != keep {
                // SAFETY: closing a descriptor this process does not use.
                unsafe { libc::close(descriptor) };
            }
        }
    }
}

fn setns(fd: RawFd, flag: libc::c_int) -> Result<(), ()> {
    // SAFETY: the descriptor names a namespace of the requested kind.
    if unsafe { libc::setns(fd, flag) } < 0 {
        return Err(());
    }
    Ok(())
}

fn exit(status: i32) -> ! {
    // SAFETY: _exit is async-signal-safe and never returns.
    unsafe { libc::_exit(status) }
}

fn exit_code(status: i32) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        1
    }
}

fn pipe() -> Result<(std::fs::File, std::fs::File), Error> {
    let mut fds = [0; 2];
    // SAFETY: the array is valid for two descriptors.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(Error::failed(format!(
            "a pipe could not be created: {}",
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: the descriptors are owned by these values from here on.
    Ok(unsafe {
        (
            std::fs::File::from_raw_fd(fds[0]),
            std::fs::File::from_raw_fd(fds[1]),
        )
    })
}

fn cstrings(values: &[String]) -> Result<Vec<CString>, Error> {
    values
        .iter()
        .map(|value| {
            CString::new(value.as_str())
                .map_err(|_| Error::failed("a command argument holds a NUL byte"))
        })
        .collect()
}

fn cstr(value: &str) -> CString {
    CString::new(value).expect("a constant path has no NUL")
}

/// The step the child reported, if any.
fn read_step(mut file: std::fs::File) -> Option<Step> {
    let mut byte = [0u8; 1];
    loop {
        match file.read(&mut byte) {
            Ok(0) => return None,
            Ok(_) => return Step::from_code(byte[0]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        }
    }
}

/// Read a stream, keeping at most `cap` bytes and draining the rest.
fn read_bounded(mut file: std::fs::File, cap: usize) -> String {
    let mut kept: Vec<u8> = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut truncated = false;
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                let room = cap.saturating_sub(kept.len());
                if room == 0 {
                    truncated = true;
                    continue;
                }
                kept.extend_from_slice(&buffer[..count.min(room)]);
                truncated |= count > room;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    let mut text = String::from_utf8_lossy(&kept).into_owned();
    if truncated {
        text.push_str("\n[truncated]");
    }
    text
}

/// Serialise launches, because `fork` in a threaded process relies on the child
/// not touching a lock another thread holds.
static JOIN: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::{Step, read_bounded, step_name};
    use std::io::Write;

    #[test]
    fn every_step_has_its_own_code() {
        let steps = [
            Step::OwnUser,
            Step::Ipc,
            Step::Net,
            Step::Pid,
            Step::Mnt,
            Step::Uts,
            Step::Cgroup,
            Step::SandboxUser,
            Step::Attach,
            Step::Fork,
            Step::Chroot,
            Step::Chdir,
            Step::Streams,
            Step::NoNewPrivileges,
            Step::Credentials,
            Step::Capabilities,
            Step::Verify,
            Step::Exec,
        ];
        let mut codes: Vec<u8> = steps.iter().map(|step| step.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), steps.len());
        for step in steps {
            assert_eq!(Step::from_code(step.code()), Some(step));
        }
        assert_eq!(Step::from_code(0), Some(Step::OwnUser));
        assert_eq!(Step::from_code(u8::MAX), None);
    }

    #[test]
    fn a_status_names_the_step() {
        assert_eq!(step_name(Step::Ipc), "entering the ipc namespace");
        assert_eq!(step_name(Step::Exec), "executing the command");
    }

    #[test]
    fn output_is_bounded_but_the_stream_is_drained() {
        let (reader, mut writer) = super::pipe().expect("pipe");
        let payload = "x".repeat(64 * 1024);
        let written = payload.clone();
        let handle = std::thread::spawn(move || {
            writer.write_all(written.as_bytes()).expect("write");
            // The reader has drained the excess, so this write cannot block
            // once it is bounded.
        });
        let text = read_bounded(reader, 1024);
        handle.join().expect("writer");
        assert!(text.starts_with(&"x".repeat(1024)), "the head is kept");
        assert!(text.ends_with("[truncated]"), "{text}");
    }
}
