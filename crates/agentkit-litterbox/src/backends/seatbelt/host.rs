//! The host reads and writes the backend needs, behind one trait.
//!
//! `SystemHost` answers from libc and the file system. `FakeHost` answers from
//! a table, so record validation, lifecycle and readiness are unit-testable
//! without a sandbox.

use std::collections::HashMap;
use std::io;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// How a process is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PidState {
    Running,
    Stopped,
    Zombie,
    Gone,
}

/// When a process started, as the kernel reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProcessStart {
    pub secs: i64,
    pub micros: u32,
}

/// What a path is, for the mount rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileKind {
    File,
    Directory,
    Socket,
    Fifo,
    Device,
}

/// A signal the backend may send to a process group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Signal {
    Stop,
    Continue,
    Kill,
}

impl Signal {
    fn raw(self) -> libc::c_int {
        match self {
            Self::Stop => libc::SIGSTOP,
            Self::Continue => libc::SIGCONT,
            Self::Kill => libc::SIGKILL,
        }
    }
}

/// Every host operation the backend performs.
pub(crate) trait Host: Send + Sync {
    fn pid_exists(&self, pid: u32) -> bool;
    fn pid_state(&self, pid: u32) -> io::Result<PidState>;
    fn process_start(&self, pid: u32) -> io::Result<ProcessStart>;
    fn signal_group(&self, pgid: u32, signal: Signal) -> io::Result<()>;
    fn connect_socket(&self, path: &Path) -> io::Result<UnixStream>;
    fn peer_uid(&self, stream: &UnixStream) -> io::Result<u32>;
    fn real_home(&self) -> io::Result<PathBuf>;
    fn realpath(&self, path: &Path) -> io::Result<PathBuf>;
    fn file_kind(&self, path: &Path) -> io::Result<FileKind>;
    fn now(&self) -> Instant;
}

/// A shared handle the backend holds.
pub(crate) type SharedHost = Arc<dyn Host>;

/// The real host.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SystemHost;

impl SystemHost {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Host for SystemHost {
    fn pid_exists(&self, pid: u32) -> bool {
        // SAFETY: signal 0 only checks whether the process exists.
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if result == 0 {
            return true;
        }
        io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }

    fn pid_state(&self, pid: u32) -> io::Result<PidState> {
        let Some(info) = process_info(pid)? else {
            return Ok(PidState::Gone);
        };
        // Darwin process states: SSTOP is 4 and SZOMB is 5. libc exposes
        // neither constant.
        match info.pbi_status {
            4 => Ok(PidState::Stopped),
            5 => Ok(PidState::Zombie),
            _ => Ok(PidState::Running),
        }
    }

    fn process_start(&self, pid: u32) -> io::Result<ProcessStart> {
        let Some(info) = process_info(pid)? else {
            return Err(io::Error::from_raw_os_error(libc::ESRCH));
        };
        Ok(ProcessStart {
            secs: info.pbi_start_tvsec as i64,
            micros: info.pbi_start_tvusec as u32,
        })
    }

    fn signal_group(&self, pgid: u32, signal: Signal) -> io::Result<()> {
        // SAFETY: a negative pid addresses the process group.
        let result = unsafe { libc::kill(-(pgid as libc::pid_t), signal.raw()) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn connect_socket(&self, path: &Path) -> io::Result<UnixStream> {
        UnixStream::connect(path)
    }

    fn peer_uid(&self, stream: &UnixStream) -> io::Result<u32> {
        use std::os::fd::AsRawFd;

        let mut uid = 0;
        let mut gid = 0;
        // SAFETY: the descriptor is open for the lifetime of the stream.
        let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
        if result == 0 {
            Ok(uid)
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn real_home(&self) -> io::Result<PathBuf> {
        super::path::real_home().map_err(|error| io::Error::other(error.to_string()))
    }

    fn realpath(&self, path: &Path) -> io::Result<PathBuf> {
        std::fs::canonicalize(path)
    }

    fn file_kind(&self, path: &Path) -> io::Result<FileKind> {
        use std::os::unix::fs::FileTypeExt;

        let metadata = std::fs::symlink_metadata(path)?;
        let kind = metadata.file_type();
        if kind.is_socket() {
            Ok(FileKind::Socket)
        } else if kind.is_fifo() {
            Ok(FileKind::Fifo)
        } else if kind.is_char_device() || kind.is_block_device() {
            Ok(FileKind::Device)
        } else if kind.is_dir() {
            Ok(FileKind::Directory)
        } else {
            Ok(FileKind::File)
        }
    }

    fn now(&self) -> Instant {
        Instant::now()
    }
}

fn process_info(pid: u32) -> io::Result<Option<libc::proc_bsdinfo>> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: the buffer is sized for the requested flavour.
    let result = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut libc::proc_bsdinfo as *mut libc::c_void,
            size,
        )
    };
    if result <= 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(None);
        }
        return Err(error);
    }
    Ok(Some(info))
}

/// A host a test fills in.
#[derive(Debug, Default)]
pub(crate) struct FakeHost {
    state: Mutex<FakeState>,
}

#[derive(Debug)]
struct FakeState {
    processes: HashMap<u32, FakeProcess>,
    signals: Vec<(u32, Signal)>,
    home: PathBuf,
    paths: HashMap<PathBuf, PathBuf>,
}

impl Default for FakeState {
    fn default() -> Self {
        Self {
            processes: HashMap::new(),
            signals: Vec::new(),
            home: PathBuf::from("/Users/tester"),
            paths: HashMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct FakeProcess {
    start: ProcessStart,
    state: PidState,
}

impl FakeHost {
    pub(crate) fn new(home: impl Into<PathBuf>) -> Self {
        Self {
            state: Mutex::new(FakeState {
                home: home.into(),
                ..FakeState::default()
            }),
        }
    }

    pub(crate) fn insert_process(&self, pid: u32, start: ProcessStart) {
        let mut state = self.state.lock().expect("fake host");
        state.processes.insert(
            pid,
            FakeProcess {
                start,
                state: PidState::Running,
            },
        );
    }

    pub(crate) fn set_state(&self, pid: u32, next: PidState) {
        let mut state = self.state.lock().expect("fake host");
        if let Some(process) = state.processes.get_mut(&pid) {
            process.state = next;
        }
    }

    pub(crate) fn signals(&self) -> Vec<(u32, Signal)> {
        self.state.lock().expect("fake host").signals.clone()
    }

    pub(crate) fn map_path(&self, from: impl Into<PathBuf>, to: impl Into<PathBuf>) {
        self.state
            .lock()
            .expect("fake host")
            .paths
            .insert(from.into(), to.into());
    }
}

impl Host for FakeHost {
    fn pid_exists(&self, pid: u32) -> bool {
        let state = self.state.lock().expect("fake host");
        state
            .processes
            .get(&pid)
            .is_some_and(|process| process.state != PidState::Gone)
    }

    fn pid_state(&self, pid: u32) -> io::Result<PidState> {
        let state = self.state.lock().expect("fake host");
        Ok(state
            .processes
            .get(&pid)
            .map_or(PidState::Gone, |process| process.state))
    }

    fn process_start(&self, pid: u32) -> io::Result<ProcessStart> {
        let state = self.state.lock().expect("fake host");
        state
            .processes
            .get(&pid)
            .map(|process| process.start)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ESRCH))
    }

    fn signal_group(&self, pgid: u32, signal: Signal) -> io::Result<()> {
        let mut state = self.state.lock().expect("fake host");
        if !state.processes.contains_key(&pgid) {
            return Err(io::Error::from_raw_os_error(libc::ESRCH));
        }
        state.signals.push((pgid, signal));
        if signal == Signal::Kill
            && let Some(process) = state.processes.get_mut(&pgid)
        {
            process.state = PidState::Gone;
        }
        Ok(())
    }

    fn connect_socket(&self, _path: &Path) -> io::Result<UnixStream> {
        UnixStream::pair().map(|(stream, _)| stream)
    }

    fn peer_uid(&self, _stream: &UnixStream) -> io::Result<u32> {
        Ok(unsafe { libc::getuid() })
    }

    fn real_home(&self) -> io::Result<PathBuf> {
        Ok(self.state.lock().expect("fake host").home.clone())
    }

    fn realpath(&self, path: &Path) -> io::Result<PathBuf> {
        let state = self.state.lock().expect("fake host");
        Ok(state
            .paths
            .get(path)
            .cloned()
            .unwrap_or_else(|| path.to_path_buf()))
    }

    fn file_kind(&self, _path: &Path) -> io::Result<FileKind> {
        Ok(FileKind::File)
    }

    fn now(&self) -> Instant {
        Instant::now()
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};

    use super::{FakeHost, FileKind, Host, PidState, ProcessStart, Signal, SystemHost};

    fn start(secs: i64) -> ProcessStart {
        ProcessStart { secs, micros: 7 }
    }

    #[test]
    fn the_fake_host_records_lifecycle() {
        let host = FakeHost::new("/Users/tester");
        host.insert_process(42, start(100));
        assert!(host.pid_exists(42));
        assert_eq!(host.pid_state(42).expect("state"), PidState::Running);
        assert_eq!(host.process_start(42).expect("start"), start(100));

        host.set_state(42, PidState::Stopped);
        assert_eq!(host.pid_state(42).expect("state"), PidState::Stopped);
        host.set_state(42, PidState::Zombie);
        assert_eq!(host.pid_state(42).expect("state"), PidState::Zombie);

        host.signal_group(42, Signal::Stop).expect("stop");
        assert_eq!(host.signals(), vec![(42, Signal::Stop)]);
        host.signal_group(42, Signal::Kill).expect("kill");
        assert_eq!(host.pid_state(42).expect("state"), PidState::Gone);
        assert!(!host.pid_exists(42));
    }

    #[test]
    fn the_fake_host_maps_paths_and_home() {
        let host = FakeHost::new("/Users/tester");
        host.map_path("/a", "/private/a");
        assert_eq!(
            host.realpath(Path::new("/a")).expect("path"),
            PathBuf::from("/private/a")
        );
        assert_eq!(
            host.real_home().expect("home"),
            PathBuf::from("/Users/tester")
        );
    }

    #[test]
    fn the_system_host_reports_itself() {
        let host = SystemHost::new();
        let pid = std::process::id();
        assert!(host.pid_exists(pid));
        assert_eq!(host.pid_state(pid).expect("state"), PidState::Running);
        assert!(host.process_start(pid).expect("start").secs > 0);
        assert!(host.real_home().expect("home").is_absolute());
        assert!(host.realpath(Path::new("/tmp")).expect("tmp").is_absolute());
        assert_eq!(
            host.file_kind(Path::new("/tmp")).expect("kind"),
            FileKind::Directory
        );
    }

    #[test]
    fn the_system_host_reads_a_peer_uid() {
        let host = SystemHost::new();
        let (stream, _peer) = UnixStream::pair().expect("pair");
        assert_eq!(host.peer_uid(&stream).expect("peer"), unsafe {
            libc::getuid()
        });
    }
}
