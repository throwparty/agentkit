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

/// Reserved exit statuses, one per join step.
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
    /// Forking the command.
    Fork,
    /// Chrooting to the pinned root.
    Chroot,
    /// Changing to the workspace.
    Chdir,
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
    /// The reserved status this step exits with.
    pub fn status(self) -> i32 {
        210 + self as i32
    }

    /// The step a reserved status names.
    pub fn from_status(status: i32) -> Option<Self> {
        const STEPS: [Step; 16] = [
            Step::OwnUser,
            Step::Ipc,
            Step::Net,
            Step::Pid,
            Step::Mnt,
            Step::Uts,
            Step::Cgroup,
            Step::SandboxUser,
            Step::Fork,
            Step::Chroot,
            Step::Chdir,
            Step::NoNewPrivileges,
            Step::Credentials,
            Step::Capabilities,
            Step::Verify,
            Step::Exec,
        ];
        STEPS.iter().copied().find(|step| step.status() == status)
    }
}

/// Run a command inside the sandbox described by the payload.
pub fn run(host: &dyn Host, data: &BubblewrapData, command: &[String]) -> Result<Execution, Error> {
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
            stdout: stdout_write.as_raw_fd(),
            stderr: stderr_write.as_raw_fd(),
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
    drop(descriptors);
    drop(root);
    drop(owner);

    let stderr_reader = std::thread::spawn(move || read_all(stderr_read));
    let mut stdout = String::new();
    let mut stdout_pipe = stdout_read;
    let _ = stdout_pipe.read_to_string(&mut stdout);
    let stderr = stderr_reader.join().unwrap_or_default();

    let mut status = 0;
    // SAFETY: the pid is the child this process forked.
    unsafe { libc::waitpid(child, &mut status, 0) };
    let exit_code = exit_code(status);

    if let Some(step) = Step::from_status(exit_code) {
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
        Step::Fork => "forking the command",
        Step::Chroot => "chrooting to the sandbox root",
        Step::Chdir => "changing to the workspace",
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
    stdout: RawFd,
    stderr: RawFd,
    arguments: &'a [CString],
    candidates: &'a [CString],
    environment: &'a [CString],
    expected: [u64; 7],
    skip: [bool; 7],
}

impl Child<'_> {
    /// Join, drop everything and exec. Never returns: it exits with a reserved
    /// status when a step fails.
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
            exit(Step::OwnUser.status());
        }
        for (step, index, flag) in order {
            if self.skip[index] {
                continue;
            }
            if setns(self.descriptors[index], flag).is_err() {
                exit(step.status());
            }
        }
        if !self.skip[0] && setns(self.descriptors[0], libc::CLONE_NEWUSER).is_err() {
            exit(Step::SandboxUser.status());
        }

        // SAFETY: the only calls between here and exec are async-signal-safe.
        let grandchild = unsafe { libc::fork() };
        if grandchild < 0 {
            exit(Step::Fork.status());
        }
        if grandchild > 0 {
            let mut status = 0;
            // SAFETY: waiting for the child this process forked.
            unsafe { libc::waitpid(grandchild, &mut status, 0) };
            exit(exit_code(status));
        }

        if unsafe { libc::dup2(self.stdout, 1) } < 0 || unsafe { libc::dup2(self.stderr, 2) } < 0 {
            exit(Step::Fork.status());
        }

        // SAFETY: fchdir and chroot operate on the descriptor this child owns.
        let dot = cstr(".");
        if unsafe { libc::fchdir(self.root) } < 0 || unsafe { libc::chroot(dot.as_ptr()) } < 0 {
            exit(Step::Chroot.status());
        }
        if unsafe { libc::chdir(cstr(WORKSPACE).as_ptr()) } < 0 {
            exit(Step::Chdir.status());
        }
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } < 0 {
            exit(Step::NoNewPrivileges.status());
        }
        // SAFETY: the ids are the ones the caller already holds.
        if unsafe { libc::setresgid(libc::getgid(), libc::getgid(), libc::getgid()) } < 0
            || unsafe { libc::setresuid(libc::getuid(), libc::getuid(), libc::getuid()) } < 0
        {
            exit(Step::Credentials.status());
        }
        if !self.capabilities_cleared() {
            exit(Step::Capabilities.status());
        }
        if !self.namespaces_match() {
            exit(Step::Verify.status());
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
        // execve does not search the PATH, so the candidates are resolved
        // against the sandbox's own PATH before the fork.
        for candidate in self.candidates {
            // SAFETY: the pointers are valid and null-terminated.
            unsafe { libc::execve(candidate.as_ptr(), argv.as_ptr(), envp.as_ptr()) };
        }
        exit(Step::Exec.status());
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

fn read_all(mut file: std::fs::File) -> String {
    let mut text = String::new();
    let _ = file.read_to_string(&mut text);
    text
}

/// Serialise joins, because `fork` in a threaded process relies on the child
/// not touching a lock another thread holds.
static JOIN: Mutex<()> = Mutex::new(());

/// Run a command while holding the join lock.
pub fn run_serialised(
    host: &dyn Host,
    data: &BubblewrapData,
    command: &[String],
) -> Result<Execution, Error> {
    let _guard = JOIN.lock().unwrap_or_else(|poison| poison.into_inner());
    run(host, data, command)
}

#[cfg(test)]
mod tests {
    use super::{Step, step_name};

    #[test]
    fn every_step_has_its_own_reserved_status() {
        let steps = [
            Step::OwnUser,
            Step::Ipc,
            Step::Net,
            Step::Pid,
            Step::Mnt,
            Step::Uts,
            Step::Cgroup,
            Step::SandboxUser,
            Step::Fork,
            Step::Chroot,
            Step::Chdir,
            Step::NoNewPrivileges,
            Step::Credentials,
            Step::Capabilities,
            Step::Verify,
            Step::Exec,
        ];
        let mut statuses: Vec<i32> = steps.iter().map(|step| step.status()).collect();
        statuses.sort_unstable();
        statuses.dedup();
        assert_eq!(statuses.len(), steps.len());
        for step in steps {
            assert_eq!(Step::from_status(step.status()), Some(step));
        }
        assert_eq!(Step::from_status(0), None);
        assert_eq!(Step::from_status(127), None);
    }

    #[test]
    fn a_status_names_the_step() {
        assert_eq!(step_name(Step::Ipc), "entering the ipc namespace");
        assert_eq!(step_name(Step::Exec), "executing the command");
    }
}
