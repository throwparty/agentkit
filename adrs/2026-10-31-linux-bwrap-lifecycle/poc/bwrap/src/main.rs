use std::env;
use std::ffi::{CString, OsString};
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const EXIT_USAGE: i32 = 64;
const EXIT_UNAVAILABLE: i32 = 69;
const EXIT_SETUP: i32 = 70;

/// Dedicated descriptor for bwrap's JSON status. Grants are opened at 3.. and
/// the harness refuses to run if that range reaches this value.
const STATUS_FD: i32 = 198;

/// The floor covers `--unshare-cgroup`, `--disable-userns`, `--die-with-parent`,
/// `--ro-bind-try` and `--perms`. 0.12.0 is the version exercised by the spike.
const VERSION_FLOOR: (u64, u64, u64) = (0, 8, 0);

/// Host trees that are never exposed as caller grants. The workspace is checked
/// separately so it may live under /tmp.
const PROTECTED: &[&str] = &["/nix", "/etc", "/run", "/proc", "/sys", "/dev", "/var"];

const ENV_EXACT: &[&str] = &[
    "PATH",
    "TERM",
    "LANG",
    "TZ",
    "CC",
    "CXX",
    "AR",
    "LD",
    "CFLAGS",
    "LDFLAGS",
    "PKG_CONFIG_PATH",
    "RUSTFLAGS",
    "RUSTC_WRAPPER",
    "SOURCE_DATE_EPOCH",
];

const ENV_PREFIX: &[&str] = &["NIX_", "CARGO_", "RUSTUP_", "SSL_CERT_", "GIT_SSL_", "LC_"];

const ENV_RESERVED: &[&str] = &["HOME", "TMPDIR", "USER", "LOGNAME", "PATH"];

struct View {
    workspace: PathBuf,
    reads: Vec<PathBuf>,
    env: Vec<(String, String)>,
    command: Vec<String>,
    print_argv: bool,
    omit: Vec<String>,
    new_session: bool,
}

fn parse() -> Result<View, String> {
    let mut args = env::args().skip(1);
    let mut workspace = None;
    let mut reads = Vec::new();
    let mut envs = Vec::new();
    let mut command = Vec::new();
    let mut print_argv = false;
    #[cfg_attr(not(feature = "attribution"), allow(unused_mut))]
    let mut omit = Vec::new();
    let mut new_session = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workspace" => {
                workspace = Some(args.next().ok_or("--workspace requires a value")?)
            }
            "--read" => reads.push(args.next().ok_or("--read requires a value")?),
            "--env" => {
                let pair = args.next().ok_or("--env requires KEY=VALUE")?;
                let (key, value) = pair
                    .split_once('=')
                    .ok_or_else(|| format!("--env expects KEY=VALUE, got {pair}"))?;
                if ENV_RESERVED.contains(&key) {
                    return Err(format!("--env {key} is reserved by the harness"));
                }
                envs.push((key.to_string(), value.to_string()));
            }
            #[cfg(feature = "attribution")]
            "--omit" => {
                let group = args.next().ok_or("--omit requires a group")?;
                if !matches!(group.as_str(), "etc" | "system" | "shell" | "store") {
                    return Err(format!("unknown omit group: {group}"));
                }
                omit.push(group);
            }
            "--print-argv" => print_argv = true,
            "--new-session" => new_session = true,
            "--" => {
                command.extend(args);
                break;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    let workspace = workspace.ok_or("--workspace is required")?;
    if command.is_empty() {
        return Err("no command given; use -- <command> [args...]".to_string());
    }

    Ok(View {
        workspace: canonical(&workspace)?,
        reads: reads.iter().map(|path| canonical(path)).collect::<Result<_, _>>()?,
        env: envs,
        command,
        print_argv,
        omit,
        new_session,
    })
}

fn canonical(raw: &str) -> Result<PathBuf, String> {
    let raw_path = Path::new(raw);
    if !raw_path.is_absolute() {
        return Err(format!("{raw}: paths must be absolute"));
    }
    if raw_path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!("{raw}: paths must not contain .."));
    }
    fs::canonicalize(raw_path).map_err(|error| format!("{raw}: {error}"))
}

fn host_home() -> Result<PathBuf, String> {
    let home = env::var_os("HOME").ok_or("HOME is not set; refusing to guess the host home")?;
    fs::canonicalize(home).map_err(|error| format!("HOME: {error}"))
}

fn overlaps(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root) || root.starts_with(path)
}

fn validate(view: &View) -> Result<(), String> {
    let home = host_home()?;

    for root in PROTECTED {
        let root = Path::new(root);
        if overlaps(&view.workspace, root) {
            return Err(format!(
                "{}: the workspace overlaps the protected path {}",
                view.workspace.display(),
                root.display()
            ));
        }
        for path in &view.reads {
            if overlaps(path, root) {
                return Err(format!(
                    "{}: read grants may not overlap the protected path {}",
                    path.display(),
                    root.display()
                ));
            }
        }
    }

    if overlaps(&view.workspace, &home) {
        return Err(format!(
            "{}: the workspace may not be inside the host home",
            view.workspace.display()
        ));
    }

    for path in &view.reads {
        if overlaps(path, &home) {
            return Err(format!(
                "{}: read grants may not touch the host home",
                path.display()
            ));
        }
        if path.starts_with("/tmp") && !path.starts_with(&view.workspace) {
            return Err(format!(
                "{}: /tmp is not a read root; use the workspace",
                path.display()
            ));
        }
    }

    Ok(())
}

#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

const O_PATH: u64 = 0o10000000;
const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;
const AT_FDCWD: i64 = -100;
const SYS_OPENAT2: i64 = 437;

unsafe extern "C" {
    fn syscall(number: i64, ...) -> i64;
    fn dup2(old: i32, new: i32) -> i32;
    fn setsid() -> i32;
}

/// Open the grant once, refusing every symlink in the path. The returned
/// descriptor is what bwrap mounts, so a path swapped after validation cannot
/// change the object inside the sandbox.
fn open_grant(path: &Path) -> Result<OwnedFd, String> {
    let raw = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| format!("{}: path contains a NUL byte", path.display()))?;
    let how = OpenHow {
        flags: O_PATH,
        mode: 0,
        resolve: RESOLVE_NO_MAGICLINKS | RESOLVE_NO_SYMLINKS,
    };
    let fd = unsafe {
        syscall(
            SYS_OPENAT2,
            AT_FDCWD,
            raw.as_ptr(),
            &how as *const OpenHow,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if fd < 0 {
        return Err(format!(
            "{}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

fn fd_source(fd: &OwnedFd) -> String {
    format!("/proc/self/fd/{}", fd.as_raw_fd())
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| fs::canonicalize(candidate).ok())
}

fn resolve_bwrap() -> Result<PathBuf, String> {
    let path = env::var_os("PATH").ok_or("PATH is not set")?;
    let mut rejection = None;
    for dir in env::split_paths(&path) {
        if !dir.is_absolute() {
            continue;
        }
        let candidate = dir.join("bwrap");
        if !candidate.is_file() {
            continue;
        }
        let resolved = match fs::canonicalize(&candidate) {
            Ok(path) => path,
            Err(error) => {
                rejection.get_or_insert(format!("{}: {error}", candidate.display()));
                continue;
            }
        };
        let metadata = match fs::metadata(&resolved) {
            Ok(metadata) => metadata,
            Err(error) => {
                rejection.get_or_insert(format!("{}: {error}", resolved.display()));
                continue;
            }
        };
        if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            rejection.get_or_insert(format!(
                "{}: bwrap must be root-owned and not group or world writable",
                resolved.display()
            ));
            continue;
        }
        return Ok(resolved);
    }
    Err(match rejection {
        Some(reason) => format!(
            "{reason}; install bubblewrap >= {}.{}.{} from a system package or nixpkgs so the binary is root-owned",
            VERSION_FLOOR.0, VERSION_FLOOR.1, VERSION_FLOOR.2
        ),
        None => format!(
            "bwrap not found on PATH; install bubblewrap >= {}.{}.{}",
            VERSION_FLOOR.0, VERSION_FLOOR.1, VERSION_FLOOR.2
        ),
    })
}

fn parse_version(output: &str) -> Option<(u64, u64, u64)> {
    output.split_whitespace().find_map(|token| {
        let core = token.trim_start_matches('v');
        let core = core.split(['-', '+']).next()?;
        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
        Some((major, minor, patch))
    })
}

fn check_version(bwrap: &Path) -> Result<(), String> {
    let output = Command::new(bwrap)
        .arg("--version")
        .output()
        .map_err(|error| format!("{}: {error}", bwrap.display()))?;
    if !output.status.success() {
        return Err(format!("{} --version failed", bwrap.display()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let version = parse_version(&stdout).ok_or_else(|| {
        format!(
            "cannot parse bwrap version from {stdout:?}; at least {}.{}.{} is required",
            VERSION_FLOOR.0, VERSION_FLOOR.1, VERSION_FLOOR.2
        )
    })?;
    if version < VERSION_FLOOR {
        return Err(format!(
            "bwrap {}.{}.{} is below the required {}.{}.{}",
            version.0, version.1, version.2, VERSION_FLOOR.0, VERSION_FLOOR.1, VERSION_FLOOR.2
        ));
    }
    Ok(())
}

fn env_allowed(key: &str) -> bool {
    ENV_EXACT.contains(&key) || ENV_PREFIX.iter().any(|prefix| key.starts_with(prefix))
}

fn allowed_host_env() -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = env::vars()
        .filter(|(key, _)| env_allowed(key))
        .collect();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn omitted(view: &View, group: &str) -> bool {
    cfg!(feature = "attribution") && view.omit.iter().any(|entry| entry == group)
}

fn build_argv(view: &View, bwrap: &Path, grants: &[OwnedFd]) -> Result<Vec<OsString>, String> {
    let env = find_in_path("env");
    let bash = find_in_path("bash");

    let workspace = grants
        .first()
        .map(fd_source)
        .ok_or("the workspace descriptor is missing")?;
    let reads = &grants[1..];

    let mut argv: Vec<OsString> = vec![
        bwrap.as_os_str().to_owned(),
        "--die-with-parent".into(),
        "--unshare-user".into(),
        "--disable-userns".into(),
        "--unshare-pid".into(),
        "--unshare-ipc".into(),
        "--unshare-uts".into(),
        "--unshare-cgroup".into(),
        "--unshare-net".into(),
        "--hostname".into(),
        "litterbox".into(),
        "--clearenv".into(),
        "--setenv".into(),
        "HOME".into(),
        "/home/sandbox".into(),
        "--setenv".into(),
        "TMPDIR".into(),
        "/tmp".into(),
        "--setenv".into(),
        "USER".into(),
        "sandbox".into(),
        "--setenv".into(),
        "LOGNAME".into(),
        "sandbox".into(),
    ];

    if view.new_session {
        argv.push("--new-session".into());
    }

    for (key, value) in allowed_host_env() {
        argv.push("--setenv".into());
        argv.push(key.into());
        argv.push(value.into());
    }

    for (key, value) in &view.env {
        argv.push("--setenv".into());
        argv.push(key.into());
        argv.push(value.into());
    }

    argv.extend([
        "--perms".into(),
        "1777".into(),
        "--tmpfs".into(),
        "/tmp".into(),
        "--tmpfs".into(),
        "/home".into(),
        "--dir".into(),
        "/home/sandbox".into(),
        "--proc".into(),
        "/proc".into(),
        "--dev".into(),
        "/dev".into(),
    ]);

    if !omitted(view, "store") {
        argv.push("--ro-bind-try".into());
        argv.push("/nix/store".into());
        argv.push("/nix/store".into());
    }

    if !omitted(view, "system") {
        for path in ["/run/current-system", "/etc/static"] {
            argv.push("--ro-bind-try".into());
            argv.push(path.into());
            argv.push(path.into());
        }
    }

    if !omitted(view, "etc") {
        for path in [
            "/etc/hosts",
            "/etc/resolv.conf",
            "/etc/nsswitch.conf",
            "/etc/passwd",
            "/etc/group",
            "/etc/ssl",
            "/etc/localtime",
        ] {
            argv.push("--ro-bind-try".into());
            argv.push(path.into());
            argv.push(path.into());
        }
    }

    if !omitted(view, "shell") {
        let nixos = Path::new("/nix/store").is_dir();
        if !nixos {
            for path in ["/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc/ld.so.cache"] {
                argv.push("--ro-bind-try".into());
                argv.push(path.into());
                argv.push(path.into());
            }
        }

        let env = env.ok_or("cannot find env on PATH")?;
        let bash = bash.ok_or("cannot find bash on PATH")?;

        argv.push("--dir".into());
        argv.push("/usr/bin".into());
        argv.push("--ro-bind".into());
        argv.push(env.into_os_string());
        argv.push("/usr/bin/env".into());

        argv.push("--dir".into());
        argv.push("/bin".into());
        argv.push("--ro-bind".into());
        argv.push(bash.into_os_string());
        argv.push("/bin/sh".into());
    }

    argv.push("--bind".into());
    argv.push(workspace.into());
    argv.push("/src".into());

    for (path, source) in view.reads.iter().zip(reads.iter()) {
        argv.push("--ro-bind".into());
        argv.push(fd_source(source).into());
        argv.push(path.as_os_str().to_owned());
    }

    argv.push("--json-status-fd".into());
    argv.push(STATUS_FD.to_string().into());

    argv.push("--chdir".into());
    argv.push("/src".into());
    argv.push("--".into());
    argv.extend(view.command.iter().map(OsString::from));
    Ok(argv)
}

fn print_argv(argv: &[OsString]) {
    let mut index = 0;
    while index < argv.len() {
        if argv[index] == "--setenv" && index + 2 < argv.len() {
            println!("--setenv");
            println!("{}", argv[index + 1].to_string_lossy());
            println!("<ENV:{}>", argv[index + 1].to_string_lossy());
            index += 3;
            continue;
        }
        println!("{}", argv[index].to_string_lossy());
        index += 1;
    }
}

fn become_session_leader() -> Result<(), String> {
    let result = unsafe { setsid() };
    if result == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(1) {
            return Ok(());
        }
        return Err(format!("setsid: {error}"));
    }
    Ok(())
}

/// Start bwrap with a status descriptor. bwrap writes a `child-pid` object once
/// the child exists; if the descriptor closes first, setup failed and no command
/// ran. That separates a confinement failure from a workload exit.
fn run_bwrap(argv: &[OsString]) -> Result<i32, String> {
    let (reader, writer) = UnixStream::pair().map_err(|error| format!("status pipe: {error}"))?;
    reader
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| format!("status timeout: {error}"))?;
    let write_fd = writer.as_raw_fd();

    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    unsafe {
        command.pre_exec(move || {
            if dup2(write_fd, STATUS_FD) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start bwrap: {error}"))?;
    drop(writer);

    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => {
            let status = child.wait().map_err(|error| format!("wait: {error}"))?;
            Err(format!("bwrap setup failed before the child started ({status})"))
        }
        Ok(_) if line.contains("child-pid") => {
            let status = child.wait().map_err(|error| format!("wait: {error}"))?;
            Ok(status.code().unwrap_or(1))
        }
        Ok(_) => {
            let status = child.wait().map_err(|error| format!("wait: {error}"))?;
            Err(format!("unexpected bwrap status line {line:?} ({status})"))
        }
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(format!("reading bwrap status: {error}"))
        }
    }
}

fn main() {
    let view = match parse() {
        Ok(view) => view,
        Err(error) => {
            eprintln!("lbx-bwrap: {error}");
            std::process::exit(EXIT_USAGE);
        }
    };

    if let Err(error) = validate(&view) {
        eprintln!("lbx-bwrap: {error}");
        std::process::exit(EXIT_USAGE);
    }

    let bwrap = match resolve_bwrap() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("lbx-bwrap: {error}");
            std::process::exit(EXIT_UNAVAILABLE);
        }
    };

    if let Err(error) = check_version(&bwrap) {
        eprintln!("lbx-bwrap: {error}");
        std::process::exit(EXIT_UNAVAILABLE);
    }

    let mut grants = Vec::with_capacity(1 + view.reads.len());
    if let Err(error) = open_grant(&view.workspace).map(|fd| grants.push(fd)) {
        eprintln!("lbx-bwrap: {error}");
        std::process::exit(EXIT_USAGE);
    }
    for path in &view.reads {
        match open_grant(path) {
            Ok(fd) => grants.push(fd),
            Err(error) => {
                eprintln!("lbx-bwrap: {error}");
                std::process::exit(EXIT_USAGE);
            }
        }
    }

    if grants
        .iter()
        .any(|fd| fd.as_raw_fd() >= STATUS_FD)
    {
        eprintln!("lbx-bwrap: too many grants for the status descriptor");
        std::process::exit(EXIT_USAGE);
    }

    let argv = match build_argv(&view, &bwrap, &grants) {
        Ok(argv) => argv,
        Err(error) => {
            eprintln!("lbx-bwrap: {error}");
            std::process::exit(EXIT_UNAVAILABLE);
        }
    };

    if view.print_argv {
        print_argv(&argv);
        return;
    }

    if let Err(error) = become_session_leader() {
        eprintln!("lbx-bwrap: {error}");
        std::process::exit(EXIT_SETUP);
    }

    match run_bwrap(&argv) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("lbx-bwrap: {error}");
            std::process::exit(EXIT_SETUP);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_version_shapes() {
        assert_eq!(parse_version("bubblewrap 0.12.0"), Some((0, 12, 0)));
        assert_eq!(parse_version("0.12.0"), Some((0, 12, 0)));
        assert_eq!(parse_version("bubblewrap 0.11.0-dev"), Some((0, 11, 0)));
        assert_eq!(parse_version("0.9.1.1"), Some((0, 9, 1)));
        assert_eq!(parse_version("not a version"), None);
    }

    #[test]
    fn containment_is_bidirectional() {
        assert!(overlaps(Path::new("/nix/store/x"), Path::new("/nix")));
        assert!(overlaps(Path::new("/nix"), Path::new("/nix/store/x")));
        assert!(!overlaps(Path::new("/nix2"), Path::new("/nix")));
    }

    #[test]
    fn relative_and_parent_paths_are_rejected() {
        assert!(canonical("relative/path").is_err());
        assert!(canonical("/tmp/../etc").is_err());
    }

    #[test]
    fn environment_allowlist_excludes_secrets() {
        assert!(env_allowed("NIX_CC"));
        assert!(env_allowed("PATH"));
        assert!(!env_allowed("OPENAI_API_KEY"));
        assert!(!env_allowed("GITHUB_TOKEN"));
    }
}
