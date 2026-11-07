//! Launch a workload with the contributions applied.
//!
//! The launcher never learns a kind: it is given a view and an environment, and
//! would be identical for any step that produced the same two contributions.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use crate::failure::Failure;

/// The directory the sandbox calls home, and the only writable path beside the
/// workspace.
const SANDBOX_HOME: &str = "/home/sandbox";

/// Files a workload needs that the store does not carry, bound one by one.
///
/// `/etc` is never bound wholesale: that would put the host's nix configuration
/// and anything else a local administrator keeps there inside the sandbox.
const HOST_FILES: [&str; 6] = [
    "/etc/passwd",
    "/etc/group",
    "/etc/nsswitch.conf",
    "/etc/resolv.conf",
    "/etc/hosts",
    "/etc/ssl/certs",
];

/// The most argv the harness will build, well inside a typical ARG_MAX.
pub const ARGV_LIMIT: usize = 1024 * 1024;

/// Paths the launcher creates inside the sandbox that no host path provides.
const SYNTHETIC: [&str; 4] = ["/dev", "/home/sandbox", "/proc", "/tmp"];

/// What a sandbox sees: the store view, the host files, and the workspace.
#[derive(Debug, Clone)]
pub struct View {
    pub reads: Vec<PathBuf>,
    pub workspace: PathBuf,
    pub whole: bool,
}

impl View {
    /// The view the backend will actually create, so a requirement is checked
    /// against the same list that is mounted.
    pub fn new(closure: Vec<PathBuf>, workspace: &Path, whole: bool) -> Result<Self, Failure> {
        let workspace = canonical_workspace(workspace)?;
        let mut reads = closure;
        for file in HOST_FILES {
            let path = PathBuf::from(file);
            if path.exists() {
                reads.push(path);
            }
        }
        reads.sort();
        reads.dedup();
        Ok(Self {
            reads,
            workspace,
            whole,
        })
    }

    /// Whether the sandbox will have this path, whether it is bound from the
    /// host or created by the launcher itself.
    pub fn provides(&self, path: &Path) -> bool {
        let synthetic = SYNTHETIC.iter().any(|dir| {
            let dir = Path::new(dir);
            path == dir || path.starts_with(dir)
        });
        synthetic
            || self
                .reads
                .iter()
                .any(|mount| path == mount || path.starts_with(mount))
            || path == self.workspace
            || path.starts_with(&self.workspace)
    }

    /// Whether the launcher creates this path rather than binding a host one.
    ///
    /// A path inside the workspace is never synthetic, even when the workspace
    /// happens to live under one of these directories: the workspace is a real
    /// bind, so what it holds is what the host holds.
    pub fn is_synthetic(&self, path: &Path) -> bool {
        if path == self.workspace || path.starts_with(&self.workspace) {
            return false;
        }
        SYNTHETIC.iter().any(|dir| {
            let dir = Path::new(dir);
            path == dir || path.starts_with(dir)
        })
    }

    /// The synthetic directories, so a report can say what the launcher adds.
    pub fn synthetic(&self) -> &'static [&'static str] {
        &SYNTHETIC
    }

    /// The store view in force, for the doctor and the record.
    pub fn store_view(&self) -> String {
        let launcher = self.synthetic().len();
        if self.whole {
            format!("store view: whole (/nix/store), {launcher} launcher paths")
        } else {
            format!(
                "store view: closure ({} store paths), {} host files, {launcher} launcher paths",
                self.store_paths(),
                self.reads.len() - self.store_paths()
            )
        }
    }

    fn store_paths(&self) -> usize {
        self.reads
            .iter()
            .filter(|path| path.starts_with("/nix/store"))
            .count()
    }
}

/// A workspace must be a real directory, absolute, and neither root nor home.
pub fn canonical_workspace(workspace: &Path) -> Result<PathBuf, Failure> {
    if !workspace.is_absolute() {
        return Err(Failure::misconfigured(format!(
            "the workspace '{}' is not an absolute path",
            workspace.display()
        ))
        .with_remedy("pass the project's absolute path"));
    }
    let canonical = workspace.canonicalize().map_err(|error| {
        Failure::misconfigured(format!(
            "the workspace '{}' does not resolve: {error}",
            workspace.display()
        ))
        .with_remedy("pass a directory that exists")
    })?;
    if canonical == Path::new("/") {
        return Err(Failure::misconfigured(
            "the workspace is the filesystem root, which would bind the host read-write into the sandbox",
        )
        .with_remedy("pass the project root, not /"));
    }
    if let Ok(home) = std::env::var("HOME")
        && canonical == Path::new(&home)
    {
        return Err(Failure::misconfigured(
            "the workspace is the home directory, which would bind every credential file into the sandbox",
        )
        .with_remedy("pass the project root, not $HOME"));
    }
    Ok(canonical)
}

/// The bwrap argv for a view, an environment and a command.
pub fn argv(view: &View, command: &[String]) -> Vec<OsString> {
    let mut argv: Vec<OsString> = Vec::new();
    for arg in [
        "--unshare-user",
        "--unshare-pid",
        "--unshare-ipc",
        "--unshare-uts",
        "--unshare-cgroup",
        "--unshare-net",
        "--disable-userns",
        "--die-with-parent",
        // Untrusted code keeps no controlling terminal, so it cannot type into
        // the shell that launched it.
        "--new-session",
    ] {
        argv.push(arg.into());
    }
    for path in &view.reads {
        argv.push("--ro-bind".into());
        argv.push(path.clone().into());
        argv.push(path.clone().into());
    }
    for (flag, path) in [("--proc", "/proc"), ("--dev", "/dev"), ("--tmpfs", "/tmp")] {
        argv.push(flag.into());
        argv.push(path.into());
    }
    argv.push("--dir".into());
    argv.push(SANDBOX_HOME.into());
    // Only the values the sandbox itself fixes go on the command line. The
    // contributed environment travels through bwrap's own environment, which
    // only its owner can read, rather than through argv, which anyone can.
    for (name, value) in [("HOME", SANDBOX_HOME), ("TMPDIR", "/tmp")] {
        argv.push("--setenv".into());
        argv.push(name.into());
        argv.push(value.into());
    }
    argv.push("--bind".into());
    argv.push(view.workspace.clone().into());
    argv.push(view.workspace.clone().into());
    argv.push("--chdir".into());
    argv.push(view.workspace.clone().into());
    argv.push("--".into());
    for arg in command {
        argv.push(arg.into());
    }
    argv
}

/// The size an argv costs, so a backend can compare it with its own limit.
pub fn argv_bytes(argv: &[OsString]) -> usize {
    argv.iter().map(|arg| arg.len() + 1).sum()
}

/// Refuse a view whose argv is larger than the launcher will attempt.
pub fn check_size(argv: &[OsString]) -> Result<usize, Failure> {
    let size = argv_bytes(argv);
    if size > ARGV_LIMIT {
        return Err(Failure::unsupported(format!(
            "the mount set needs {size} bytes of argv, more than the {ARGV_LIMIT}-byte limit"
        ))
        .with_remedy("select the whole store view, or use a backend that does not pass mounts as arguments"));
    }
    Ok(size)
}

pub fn launch(
    bwrap: &Path,
    view: &View,
    environment: &BTreeMap<String, String>,
    command: &[String],
) -> Result<(ExitStatus, usize), Failure> {
    let argv = argv(view, command);
    let size = check_size(&argv)?;
    let status = Command::new(bwrap)
        .args(&argv)
        .env_clear()
        .envs(environment)
        .status()
        .map_err(|error| Failure::failed(format!("could not run bwrap: {error}")))?;
    Ok((status, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(reads: &[&str]) -> View {
        // Not temp_dir itself: a workspace at /tmp would make every /tmp path
        // workspace-rooted, which is the opposite of what these tests assert.
        let workspace = std::env::temp_dir()
            .join(format!("devshell-poc-view-{}", std::process::id()))
            .join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let workspace = workspace.canonicalize().expect("workspace resolves");
        View::new(
            reads.iter().map(PathBuf::from).collect(),
            &workspace,
            false,
        )
        .expect("a view")
    }

    fn bound(argv: &[OsString], path: &str) -> usize {
        let argv: Vec<String> = argv
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        argv.windows(3)
            .filter(|window| window[0] == "--ro-bind" && window[1] == path && window[2] == path)
            .count()
    }

    #[test]
    fn a_closure_path_is_bound_once_and_only_once() {
        let view = view(&["/nix/store/aaa-dev", "/nix/store/aaa-dev"]);
        let argv = argv(&view, &["true".to_string()]);
        assert_eq!(bound(&argv, "/nix/store/aaa-dev"), 1);
    }

    #[test]
    fn the_whole_store_view_is_one_bind() {
        let workspace = std::env::temp_dir().canonicalize().expect("temp resolves");
        let view = View::new(vec![PathBuf::from("/nix/store")], &workspace, true).expect("a view");
        let argv = argv(&view, &["true".to_string()]);
        let store_binds = argv
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .filter(|arg| arg == "/nix/store")
            .count();
        assert_eq!(store_binds, 2, "one source and one destination for the root");
        assert!(view.store_view().contains("whole"));
    }

    #[test]
    fn etc_is_never_bound_wholesale() {
        let view = view(&["/nix/store/aaa-dev"]);
        let argv = argv(&view, &["true".to_string()]);
        assert_eq!(bound(&argv, "/etc"), 0, "/etc is not a mount");
        assert_eq!(bound(&argv, "/run/current-system"), 0);
    }

    #[test]
    fn the_sandbox_gets_a_new_session() {
        let view = view(&["/nix/store/aaa-dev"]);
        let argv = argv(&view, &["true".to_string()]);
        let argv: Vec<String> = argv.iter().map(|a| a.to_string_lossy().to_string()).collect();
        assert!(argv.iter().any(|arg| arg == "--new-session"));
        assert!(argv.iter().any(|arg| arg == "--die-with-parent"));
    }

    #[test]
    fn a_contributed_value_never_appears_in_argv() {
        let view = view(&["/nix/store/aaa-dev"]);
        let mut environment = BTreeMap::new();
        environment.insert("GITHUB_TOKEN".to_string(), "fixture-secret".to_string());
        // argv is world-readable, so it must not carry the value; the
        // environment is handed to bwrap through its own environment.
        let argv: Vec<String> = argv(&view, &["true".to_string()])
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        assert!(
            !argv.iter().any(|arg| arg.contains("fixture-secret")),
            "the contributed value reached argv: {argv:?}"
        );
        assert!(
            !argv.iter().any(|arg| arg == "GITHUB_TOKEN"),
            "a contributed name reached argv: {argv:?}"
        );
        assert!(environment.contains_key("GITHUB_TOKEN"));
    }

    #[test]
    fn only_the_sandbox_defaults_are_set_on_the_command_line() {
        let view = view(&["/nix/store/aaa-dev"]);
        let argv: Vec<String> = argv(&view, &["true".to_string()])
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        let setenv: Vec<String> = argv
            .windows(3)
            .filter(|window| window[0] == "--setenv")
            .map(|window| format!("{}={}", window[1], window[2]))
            .collect();
        assert_eq!(setenv, vec!["HOME=/home/sandbox", "TMPDIR=/tmp"]);
    }

    #[test]
    fn the_launcher_creates_the_paths_it_reports_as_synthetic() {
        let view = view(&["/nix/store/aaa-dev"]);
        assert!(view.provides(Path::new("/tmp/scratch")));
        assert!(view.provides(Path::new("/dev/null")));
        assert!(view.provides(Path::new("/proc/self")));
        assert!(view.provides(Path::new("/home/sandbox/.cargo")));
        assert!(view.is_synthetic(Path::new("/tmp")));
        assert!(!view.is_synthetic(Path::new("/etc/hosts")));
    }

    #[test]
    fn the_workspace_is_bound_read_write_and_is_the_working_directory() {
        let view = view(&["/nix/store/aaa-dev"]);
        let argv = argv(&view, &["true".to_string()]);
        let argv: Vec<String> = argv.iter().map(|a| a.to_string_lossy().to_string()).collect();
        assert!(argv.windows(3).any(|window| window[0] == "--bind"));
        assert!(argv.iter().any(|arg| arg == "--chdir"));
    }

    #[test]
    fn an_oversized_view_is_refused_naming_the_limit() {
        let huge: Vec<OsString> = (0..200_000)
            .map(|index| OsString::from(format!("--ro-bind-{index}")))
            .collect();
        let failure = check_size(&huge).unwrap_err();
        assert_eq!(failure.class, crate::failure::Class::Unsupported);
        assert!(failure.reason.contains(&ARGV_LIMIT.to_string()), "{}", failure.reason);
    }

    #[test]
    fn a_small_view_reports_its_size() {
        let argv = vec![OsString::from("a"), OsString::from("bb")];
        assert_eq!(check_size(&argv).expect("small"), 5);
    }

    #[test]
    fn the_root_is_refused_as_a_workspace() {
        let failure = canonical_workspace(Path::new("/")).unwrap_err();
        assert_eq!(failure.class, crate::failure::Class::Misconfigured);
    }

    #[test]
    fn a_relative_workspace_is_refused() {
        let failure = canonical_workspace(Path::new("relative/path")).unwrap_err();
        assert!(failure.reason.contains("not an absolute path"));
    }

    #[test]
    fn a_missing_workspace_is_refused() {
        let failure = canonical_workspace(Path::new("/definitely/not/here")).unwrap_err();
        assert!(failure.reason.contains("does not resolve"));
    }
}
