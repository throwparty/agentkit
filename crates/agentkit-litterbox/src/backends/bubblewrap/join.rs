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
use std::time::{Duration, Instant};

use super::BACKEND;
use super::backend::Network;
use super::host::{Host, Namespace};
use super::payload::Payload;
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
    data: &Payload,
    command: &[String],
    cap: usize,
) -> Result<Execution, Error> {
    let variables = host.environment(data.sandbox_pid).ok_or_else(|| {
        Error::failed("the sandbox environment can no longer be read, so no command can run in it")
    })?;

    let mut descriptors = Vec::new();
    for namespace in Namespace::ALL {
        let name = namespace.name();
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
    let user = descriptors[Namespace::User.index()].as_raw_fd();
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
    let skip = skipped(data.network);
    // Everything the child needs is built here: between fork and exec it may
    // touch neither the allocator nor any lock another thread might hold.
    let dot = cstr(".");
    let workspace_path = cstr(WORKSPACE);
    let self_namespaces = cstr("/proc/self/ns");

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
            self_namespaces: &self_namespaces,
            dot: &dot,
            workspace: &workspace_path,
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
    // command finishes. It is bounded, so a launch that wedges cannot hold the
    // join lock for the life of the server.
    let failure = match read_step(status_read, Instant::now() + LAUNCH_TIMEOUT) {
        Ok(failure) => failure,
        Err(error) => {
            // The pid this process owns is the waiter; the command it forked
            // asks the kernel to kill it when the waiter goes, so both are
            // reaped rather than one left holding the pipes.
            // SAFETY: the pid is the child this process forked.
            unsafe { libc::kill(child, libc::SIGKILL) };
            // SAFETY: the pid is the child this process forked.
            unsafe { libc::waitpid(child, std::ptr::null_mut(), 0) };
            // The readers end when the last writer is gone; joining them keeps
            // their threads from outliving the join.
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(Error::failed(format!(
                "{BACKEND}: the join did not report within {}s: {error}",
                LAUNCH_TIMEOUT.as_secs()
            )));
        }
    };
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

/// Which namespaces the child skips entering.
///
/// Only the network namespace is skipped, and only when the sandbox shares the
/// host's. That namespace is owned by the initial user namespace, where this
/// process has no capability, so `setns` would fail with `EPERM`. Every other
/// namespace is entered and all seven are checked against the record.
fn skipped(network: Network) -> [bool; 7] {
    let mut skip = [false; 7];
    skip[Namespace::Net.index()] = network == Network::Host;
    skip
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
    self_namespaces: &'a CString,
    dot: &'a CString,
    workspace: &'a CString,
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
            (Step::Ipc, Namespace::Ipc, libc::CLONE_NEWIPC),
            (Step::Net, Namespace::Net, libc::CLONE_NEWNET),
            (Step::Pid, Namespace::Pid, libc::CLONE_NEWPID),
            (Step::Mnt, Namespace::Mnt, libc::CLONE_NEWNS),
            (Step::Uts, Namespace::Uts, libc::CLONE_NEWUTS),
            (Step::Cgroup, Namespace::Cgroup, libc::CLONE_NEWCGROUP),
        ];

        if setns(self.owner, libc::CLONE_NEWUSER).is_err() {
            self.fail(Step::OwnUser);
        }
        // The scope is joined before the cgroup namespace is entered: from
        // inside that namespace this process's own cgroup is outside its root,
        // and the kernel refuses to migrate a process whose cgroup it cannot
        // see. It is also joined before the sandbox's own user namespace, which
        // does not carry the capability the cgroup namespace needs.
        if let Some(scope) = self.scope
            && !attach(scope)
        {
            self.fail(Step::Attach);
        }
        for (step, namespace, flag) in order {
            let index = namespace.index();
            if self.skip[index] {
                continue;
            }
            if setns(self.descriptors[index], flag).is_err() {
                self.fail(step);
            }
        }
        if !self.skip[Namespace::User.index()]
            && setns(
                self.descriptors[Namespace::User.index()],
                libc::CLONE_NEWUSER,
            )
            .is_err()
        {
            self.fail(Step::SandboxUser);
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

        // The command dies with the waiter that forked it, so a launch that
        // wedges cannot leave a process behind after the tool has given up on
        // it. The parent is read first: if it is already gone, the signal will
        // never arrive and this process has to go by itself.
        // SAFETY: prctl and getppid have no preconditions.
        let waiter = unsafe { libc::getppid() };
        if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) } < 0 {
            self.fail(Step::Fork);
        }
        // SAFETY: as above.
        if unsafe { libc::getppid() } != waiter {
            self.fail(Step::Fork);
        }

        // The namespace directory is opened here, after the join and before
        // the chroot: the descriptor the tool holds names the tool's own
        // namespaces, so verifying against it would reject a join that worked.
        let namespaces = {
            // SAFETY: the path is a constant built before the fork and the
            // descriptor is this process's.
            unsafe {
                libc::openat(
                    libc::AT_FDCWD,
                    self.self_namespaces.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
                )
            }
        };
        if namespaces < 0 {
            self.fail(Step::Verify);
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
        if unsafe { libc::fchdir(self.root) } < 0
            || unsafe { libc::chroot(self.dot.as_ptr()) } < 0
        {
            self.fail(Step::Chroot);
        }
        // SAFETY: the path is a constant carried in from the parent.
        if unsafe { libc::chdir(self.workspace.as_ptr()) } < 0 {
            self.fail(Step::Chdir);
        }
        // SAFETY: no-new-privileges has no preconditions.
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
        if !self.namespaces_match(namespaces) {
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

    fn namespaces_match(&self, namespaces: RawFd) -> bool {
        // Read through the descriptor this process opened after the fork and
        // parse the inode by hand: allocating here could deadlock against
        // another thread's allocator lock while the join lock is held.
        let mut buffer = [0u8; 64];
        for namespace in Namespace::ALL {
            let name = namespace.c_name();
            // SAFETY: the descriptor names this process's namespace directory,
            // the name is a constant, and the buffer is writable for its whole
            // length.
            let length = unsafe {
                libc::readlinkat(
                    namespaces,
                    name.as_ptr(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                )
            };
            if length <= 0 {
                return false;
            }
            let link = &buffer[..length as usize];
            let Some(start) = link.iter().position(|byte| *byte == b'[') else {
                return false;
            };
            let Some(end) = link.iter().position(|byte| *byte == b']') else {
                return false;
            };
            if end <= start + 1 {
                return false;
            }
            let mut inode: u64 = 0;
            for byte in &link[start + 1..end] {
                if !byte.is_ascii_digit() {
                    return false;
                }
                inode = inode.saturating_mul(10).saturating_add(u64::from(byte - b'0'));
            }
            if inode != self.expected[namespace.index()] {
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

/// Read the step the child reported, if any, within the deadline.
fn read_step(mut file: std::fs::File, deadline: Instant) -> std::io::Result<Option<Step>> {
    // SAFETY: the descriptor is open for reading and F_SETFL applies to it.
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(std::io::Error::last_os_error());
    }
    let mut byte = [0u8; 1];
    loop {
        match file.read(&mut byte) {
            Ok(0) => return Ok(None),
            Ok(_) => return Ok(Step::from_code(byte[0])),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "the launch did not finish",
                    ));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => return Err(error),
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

/// How long a launch has to exec the command before it is abandoned.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(test)]
mod tests {
    use super::{Network, Step, read_bounded, skipped, step_name};
    use std::io::Write;

    #[test]
    fn only_the_host_network_namespace_is_skipped() {
        assert_eq!(skipped(Network::Denied), [false; 7]);
        assert_eq!(
            skipped(Network::Host),
            [false, false, true, false, false, false, false]
        );
    }

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
