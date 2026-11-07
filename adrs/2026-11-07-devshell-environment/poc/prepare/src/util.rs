//! Process, environment and file helpers shared by the harness.
//!
//! Every host command runs from one sanitised caller environment and under a
//! deadline, its output is bounded, and every file holding a value is private.

use std::collections::BTreeMap;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::failure::Failure;

/// The longest a single host preparation command may run.
pub const HOST_DEADLINE: Duration = Duration::from_secs(300);

/// How long stream collection waits after the child exits before killing the
/// process group that still holds a pipe.
const STREAM_GRACE: Duration = Duration::from_secs(5);

/// The most of one stream kept in memory; the rest is drained and dropped.
const CAPTURE_LIMIT: usize = 8 * 1024 * 1024;

/// How much of each end of a stream a failure excerpt carries.
const EXCERPT_BYTES: usize = 2048;

/// Names a POSIX process may have. Anything else is not an environment record.
fn is_name(name: &str) -> bool {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// A sequence so two private directories in one process never share a name.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The minimal PATH every host command receives. Binaries the harness runs are
/// resolved to absolute paths first, so this only has to serve the child's own
/// needs: the shell, coreutils and the toolchain a devShell hook calls.
const CALLER_PATH: &str = "/run/current-system/sw/bin:/usr/local/bin:/usr/bin:/bin";

/// The caller the harness supplies to every host process: HOME and nothing else.
pub struct Caller {
    home: PathBuf,
}

impl Caller {
    /// A caller whose HOME is a private directory the harness removes on drop.
    pub fn new() -> Result<Self, Failure> {
        // A PID alone is guessable and repeats within one process, so two
        // callers would share a HOME and the first drop would delete the
        // second's.
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let home = std::env::temp_dir().join(format!(
            "devshell-poc-home-{}-{sequence}",
            std::process::id()
        ));
        private_dir(&home)?;
        Ok(Self { home })
    }

    /// The environment every host command receives. Nothing of the caller's
    /// session is here, so a credential is absent because it was never present.
    pub fn env(&self) -> Vec<(String, String)> {
        vec![
            ("HOME".to_string(), self.home.display().to_string()),
            ("PATH".to_string(), CALLER_PATH.to_string()),
        ]
    }
}

impl Drop for Caller {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.home);
    }
}

/// Arguments from literals, for the fixed command lines the harness runs.
pub fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_string()).collect()
}

/// Resolve a program to an absolute path once, so a PATH entry cannot later
/// substitute the binary a host command runs.
pub fn resolve(program: &str) -> Result<PathBuf, Failure> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(program);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(Failure::unavailable(program, format!("{program} is not on PATH"))
        .with_remedy(format!("install {program} and put it on PATH")))
}

/// One finished host command, with both streams bounded.
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stderr_truncated: bool,
}

impl Output {
    pub fn success(&self) -> bool {
        self.status.success()
    }

    /// The last part of stderr, for a failure that has to stay legible.
    pub fn stderr_excerpt(&self) -> String {
        excerpt(&self.stderr, self.stderr_truncated)
    }
}

/// Run a host command under the caller environment and the harness deadline.
pub fn run(
    program: &Path,
    args: &[String],
    caller: &Caller,
    cwd: Option<&Path>,
) -> Result<Output, Failure> {
    let name = program.display().to_string();
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear()
        .envs(caller.env().iter().map(|(k, v)| (k, v)))
        .process_group(0);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }

    let mut child = command.spawn().map_err(|error| {
        Failure::unavailable(&name, format!("could not run {name}: {error}"))
            .with_remedy(format!("check that {name} is executable"))
    })?;
    let pid = child.id();
    let (stdout_sender, stdout_receiver) = mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            let _ = stdout_sender.send(collect(stdout));
        });
    }
    let (stderr_sender, stderr_receiver) = mpsc::channel();
    if let Some(stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            let _ = stderr_sender.send(collect(stderr));
        });
    }

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                return Err(Failure::failed(format!(
                    "could not wait for {name}: {error}"
                )));
            }
        }
        if started.elapsed() >= HOST_DEADLINE {
            kill_group(pid);
            let _ = child.kill();
            let _ = child.wait();
            return Err(Failure::unavailable(
                &name,
                format!(
                    "{name} did not finish within {}s and was killed",
                    HOST_DEADLINE.as_secs()
                ),
            )
            .with_remedy(format!("run {name} by hand to see what it waits on")));
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    // The child has exited, but a grandchild may still hold the write end of a
    // pipe. Waiting for EOF without a bound would let it hold preparation open
    // past the deadline, so the same deadline covers stream collection.
    let stdout = collect_stream(stdout_receiver, pid, &name, "stdout")?;
    let stderr = collect_stream(stderr_receiver, pid, &name, "stderr")?;

    Ok(Output {
        status,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
        stderr_truncated: stderr.truncated,
    })
}

#[derive(Default)]
struct Collected {
    bytes: Vec<u8>,
    truncated: bool,
}

/// Wait for a stream to be collected, killing whatever still holds the pipe
/// once the grace period expires.
fn collect_stream(
    receiver: mpsc::Receiver<Collected>,
    pid: u32,
    name: &str,
    stream: &str,
) -> Result<Collected, Failure> {
    match receiver.recv_timeout(STREAM_GRACE) {
        Ok(collected) => Ok(collected),
        Err(mpsc::RecvTimeoutError::Disconnected) => Ok(Collected::default()),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            kill_group(pid);
            receiver.recv_timeout(STREAM_GRACE).map_err(|_| {
                Failure::failed(format!(
                    "{name} exited but something still holds its {stream} open"
                ))
                .with_remedy("stop the hook from leaving a background process behind")
            })
        }
    }
}

fn collect(mut reader: impl Read) -> Collected {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut buffer = [0u8; 16 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                let room = CAPTURE_LIMIT.saturating_sub(bytes.len());
                if room == 0 {
                    truncated = true;
                    continue;
                }
                let keep = room.min(read);
                bytes.extend_from_slice(&buffer[..keep]);
                truncated |= keep < read;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    Collected { bytes, truncated }
}

/// Kill the process group the command leads, so a wedged grandchild goes too.
fn kill_group(pid: u32) {
    let _ = Command::new("sh")
        .args(["-c", "kill -9 -- -\"$1\"", "sh", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// A head-and-tail excerpt of a stream, for a message a person reads.
pub fn excerpt(bytes: &[u8], truncated: bool) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    if !truncated && text.len() <= EXCERPT_BYTES * 2 {
        return text.to_string();
    }
    let head: String = text.chars().take(EXCERPT_BYTES).collect();
    let tail: String = {
        let characters: Vec<char> = text.chars().collect();
        let start = characters.len().saturating_sub(EXCERPT_BYTES);
        characters[start..].iter().collect()
    };
    format!("{head}\n[... truncated ...]\n{tail}")
}

/// Environment records read from a NUL-separated capture.
pub struct Records {
    pub values: BTreeMap<String, String>,
    /// Records rejected as not well-formed assignments, for provenance.
    pub rejected: Vec<String>,
}

/// Read NUL-separated `NAME=value` records, keeping only well-formed names.
///
/// A hook that prints a banner cannot corrupt the environment: a line that is
/// not an assignment, or whose name is not a process name, is recorded as
/// rejected instead of entering the map.
pub fn read_nul(path: &Path) -> Result<Records, Failure> {
    let bytes = fs::read(path).map_err(|error| {
        Failure::failed(format!("could not read {}: {error}", path.display()))
    })?;
    let mut values = BTreeMap::new();
    let mut rejected = Vec::new();
    for record in bytes.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        let record = String::from_utf8_lossy(record);
        let Some((name, value)) = record.split_once('=') else {
            rejected.push(clip(&record));
            continue;
        };
        if is_name(name) {
            values.insert(name.to_string(), value.to_string());
        } else {
            rejected.push(clip(&record));
        }
    }
    Ok(Records { values, rejected })
}

fn clip(record: &str) -> String {
    record.chars().take(80).collect()
}

/// Create a directory that no one else owns, refusing a pre-existing one.
pub fn private_dir(path: &Path) -> Result<(), Failure> {
    let mut builder = DirBuilder::new();
    builder.mode(0o700);
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(Failure::misconfigured(
            format!(
                "'{}' already exists, and the harness does not reuse a directory it did not create",
                path.display()
            ),
        )
        .with_remedy("remove it, or choose a path that does not exist")),
        Err(error) => Err(Failure::failed(format!(
            "could not create {}: {error}",
            path.display()
        ))),
    }
}

/// Write NUL-separated records to a file only the caller can read.
pub fn write_nul_strings(path: &Path, values: impl Iterator<Item = String>) -> Result<(), Failure> {
    let mut file = create_private(path)?;
    for value in values {
        file.write_all(value.as_bytes())
            .and_then(|_| file.write_all(&[0]))
            .map_err(|error| Failure::failed(format!("could not write {}: {error}", path.display())))?;
    }
    Ok(())
}

pub fn write_nul_paths(path: &Path, values: &[PathBuf]) -> Result<(), Failure> {
    write_nul_strings(path, values.iter().map(|value| value.display().to_string()))
}

/// A file created 0600 that refuses to follow a symlink or reuse a path.
pub fn create_private(path: &Path) -> Result<File, Failure> {
    let _ = fs::remove_file(path);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    options
        .open(path)
        .map_err(|error| Failure::failed(format!("could not create {}: {error}", path.display())))
}

/// Write a value-bearing text file, refusing to follow a symlink.
pub fn write_private(path: &Path, text: &str) -> Result<(), Failure> {
    let mut file = create_private(path)?;
    file.write_all(text.as_bytes())
        .map_err(|error| Failure::failed(format!("could not write {}: {error}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "devshell-poc-util-{}-{name}",
            std::process::id()
        ))
    }

    #[test]
    fn a_private_directory_is_created_once() {
        let path = scratch("exclusive");
        let _ = fs::remove_dir_all(&path);
        private_dir(&path).expect("creates");
        let mode = fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        let failure = private_dir(&path).expect_err("refuses to reuse");
        assert_eq!(failure.class, crate::failure::Class::Misconfigured);
        assert!(failure.remedy.is_some());
        fs::remove_dir_all(&path).ok();
    }

    #[test]
    fn a_private_directory_does_not_adopt_a_symlink() {
        let target = scratch("symlink-target");
        let link = scratch("symlink-link");
        let _ = fs::remove_dir_all(&target);
        let _ = fs::remove_file(&link);
        fs::create_dir_all(&target).expect("target");
        symlink(&target, &link).expect("link");
        let failure = private_dir(&link).expect_err("refuses a symlink");
        assert_eq!(failure.class, crate::failure::Class::Misconfigured);
        fs::remove_file(&link).ok();
        fs::remove_dir_all(&target).ok();
    }

    #[test]
    fn two_callers_hold_different_homes() {
        let home_of = |caller: &Caller| {
            PathBuf::from(
                caller
                    .env()
                    .into_iter()
                    .find(|(name, _)| name == "HOME")
                    .map(|(_, value)| value)
                    .expect("a caller supplies HOME"),
            )
        };
        let first = Caller::new().expect("first");
        let second = Caller::new().expect("second");
        assert_ne!(home_of(&first), home_of(&second));
        let first_home = home_of(&first);
        drop(first);
        assert!(!first_home.exists(), "the dropped caller removes its own home");
        assert!(home_of(&second).exists(), "and not its neighbour's");
    }

    #[test]
    fn a_disconnected_stream_is_empty_rather_than_held_open() {
        let (sender, receiver) = mpsc::channel();
        drop(sender);
        let collected = collect_stream(receiver, 0, "test", "stdout").expect("empty");
        assert!(collected.bytes.is_empty());
    }

    #[test]
    fn a_record_name_is_a_process_name() {
        assert!(is_name("PATH"));
        assert!(is_name("_"));
        assert!(is_name("CARGO_HOME"));
        assert!(!is_name(""));
        assert!(!is_name("==> building <=="), "a banner line is not a name");
        assert!(!is_name("2fast"));
        assert!(!is_name("has-dash"));
        assert!(!is_name("has space"));
    }

    #[test]
    fn a_banner_record_is_rejected_not_contributed() {
        let path = scratch("banner.nul");
        let mut bytes = Vec::new();
        for record in ["PATH=/bin", "==> building <==", "LOG=1"] {
            bytes.extend_from_slice(record.as_bytes());
            bytes.push(0);
        }
        fs::write(&path, &bytes).expect("fixture writes");
        let records = read_nul(&path).expect("reads");
        fs::remove_file(&path).ok();
        assert_eq!(records.values.get("PATH").map(String::as_str), Some("/bin"));
        assert_eq!(records.values.get("LOG").map(String::as_str), Some("1"));
        assert_eq!(records.rejected.len(), 1, "{:?}", records.rejected);
        assert!(records.rejected[0].contains("==> building <=="));
    }

    #[test]
    fn a_record_without_an_assignment_is_rejected() {
        let path = scratch("plain.nul");
        fs::write(&path, b"just a line\0GOOD=1\0").expect("fixture writes");
        let records = read_nul(&path).expect("reads");
        fs::remove_file(&path).ok();
        assert_eq!(records.values.len(), 1);
        assert_eq!(records.rejected, vec!["just a line".to_string()]);
    }

    #[test]
    fn a_value_file_is_private_and_does_not_follow_a_symlink() {
        let victim = scratch("victim");
        let link = scratch("env.nul");
        fs::write(&victim, b"untouched").expect("fixture writes");
        let _ = fs::remove_file(&link);
        symlink(&victim, &link).expect("fixture links");

        write_nul_strings(&link, ["SECRET=1".to_string()].into_iter()).expect("writes");

        let mode = fs::metadata(&link).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "value files are readable only by their owner");
        assert_eq!(fs::read(&victim).expect("victim reads"), b"untouched");
        assert_eq!(
            fs::read(&link).expect("link reads"),
            b"SECRET=1\0",
            "the symlink itself was replaced, not followed"
        );
        fs::remove_file(&victim).ok();
        fs::remove_file(&link).ok();
    }

    #[test]
    fn an_excerpt_keeps_the_head_and_the_tail() {
        let long = format!("{}\n{}", "start".repeat(1000), "end".repeat(1000));
        let rendered = excerpt(long.as_bytes(), true);
        assert!(rendered.starts_with("start"));
        assert!(rendered.ends_with("end"));
        assert!(rendered.contains("truncated"), "{rendered}");
    }

    #[test]
    fn a_short_excerpt_is_verbatim() {
        assert_eq!(excerpt(b"sh: nix: not found\n", false), "sh: nix: not found");
    }

    #[test]
    fn a_missing_program_is_unavailable() {
        let failure = resolve("devshell-poc-not-a-program").unwrap_err();
        assert_eq!(failure.class, crate::failure::Class::Unavailable);
        assert!(failure.remedy.is_some());
    }
}
