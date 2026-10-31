//! Throwaway prototype: the same confinement request expressed through nono.
//!
//! Anything nono cannot express is recorded as a gap, not worked around
//! silently. Each `platform_rule` call is such a workaround and is listed in
//! `results/t-011-nono-gaps.md`.
//!
//! The CLI, path validation and environment handling are deliberately copied
//! from the custom prototype: FR-002 forbids a shared crate. Keep them in step;
//! the canonicalise-or-reject step here is the same one the custom helper
//! applies.

use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use nono::{AccessMode, CapabilitySet, Sandbox};

unsafe extern "C" {
    fn setsid() -> i32;
    fn getpgrp() -> i32;
    fn getpid() -> i32;
    fn getsid(pid: i32) -> i32;
    fn close(fd: i32) -> i32;
}

/// Same set as the custom helper: the invariants the caller cannot override.
const CREDENTIAL_DENY: &[&str] = &[
    ".ssh",
    ".aws",
    ".gnupg",
    ".docker",
    ".kube",
    ".netrc",
    ".git-credentials",
    ".npmrc",
    ".cargo/credentials.toml",
    ".config/gh",
    ".config/gcloud",
    ".config/containers",
    ".config/pip",
    ".pypirc",
    ".terraform.d",
    ".vault-token",
    ".password-store",
    ".local/share/keyrings",
    "Library/Keychains",
    "Library/Application Support/Google/Chrome",
    "Library/Application Support/Firefox",
    "Library/Safari",
    "Library/Cookies",
];

const USAGE: i32 = 64;
const FAIL_CLOSED: i32 = 70;
const EXEC_FAILED: i32 = 126;

const USAGE_TEXT: &str = "\
usage: sandbox-nono --workspace <path> [options] -- <command> [args...]

options:
  --workspace <path>      sandbox root, read-write (required, absolute)
  --read <path>           additional read-only grant (repeatable)
  --write <path>          additional read-write grant (repeatable)
  --connect <host:port>   egress grant (repeatable; localhost or * only)
  --tag <name>            sandbox identifier for denied-operation attribution
  -h, --help              this text
";

const BASELINE_READ: &[&str] = &[
    "/usr",
    "/System/Library",
    "/System/Applications",
    "/Library/Developer",
    "/Applications/Xcode.app/Contents/Developer",
    "/Library/Frameworks",
    "/bin",
    "/sbin",
    "/private/etc",
    "/private/var/db/dyld",
    "/private/var/select",
    "/dev",
    "/nix/store",
];

struct Args {
    workspace: PathBuf,
    reads: Vec<PathBuf>,
    writes: Vec<PathBuf>,
    connects: Vec<String>,
    tag: String,
    command: Vec<String>,
}

fn parse() -> Result<Args, String> {
    let mut workspace = None;
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    let mut connects = Vec::new();
    let mut tag = "litterbox".to_string();
    let mut command = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE_TEXT}");
                std::process::exit(0);
            }
            "--workspace" => {
                workspace = Some(PathBuf::from(
                    args.next().ok_or("--workspace needs a path")?,
                ))
            }
            "--read" => reads.push(PathBuf::from(args.next().ok_or("--read needs a path")?)),
            "--write" => writes.push(PathBuf::from(args.next().ok_or("--write needs a path")?)),
            "--connect" => connects.push(args.next().ok_or("--connect needs host:port")?),
            "--tag" => tag = args.next().ok_or("--tag needs a name")?,
            "--" => {
                command.extend(args);
                break;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    Ok(Args {
        workspace: workspace.ok_or("--workspace is required")?,
        reads,
        writes,
        connects,
        tag,
        command,
    })
}

/// Same validation as the custom prototype: relative paths, `..` components
/// and symlinks escaping the workspace are rejected before anything is applied.
fn grant(path: &Path, workspace: Option<&Path>) -> Result<PathBuf, String> {
    if path.is_relative() {
        return Err(format!("{} is relative", path.display()));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!("{} contains a parent component", path.display()));
    }

    let resolved = path
        .canonicalize()
        .map_err(|err| format!("{} does not resolve: {err}", path.display()))?;

    if let Some(workspace) = workspace {
        if ancestor_reaches(path, workspace) && !resolved.starts_with(workspace) {
            return Err(format!("{} resolves outside the workspace", path.display()));
        }
    }

    Ok(resolved)
}

/// True when canonicalising any ancestor of `path` lands on `workspace`, so a
/// symlinked intermediate directory cannot smuggle a grant out.
fn ancestor_reaches(path: &Path, workspace: &Path) -> bool {
    let mut current = path.parent();
    while let Some(candidate) = current {
        if candidate.canonicalize().ok().as_deref() == Some(workspace) {
            return true;
        }
        current = candidate.parent();
    }
    false
}

fn connect(value: &str) -> Result<String, String> {
    let (host, port) = value
        .rsplit_once(':')
        .ok_or_else(|| format!("{value} is not host:port"))?;
    if host.is_empty() {
        return Err(format!("{value} has no host"));
    }
    if host != "localhost" {
        return Err(format!(
            "{value}: host must be localhost; the profile language accepts only localhost or *, and * grants every host"
        ));
    }
    let port: u16 = port
        .parse()
        .map_err(|_| format!("{value}: port is not a number"))?;
    if port == 0 {
        return Err(format!("{value}: port must not be 0"));
    }
    Ok(format!("{host}:{port}"))
}

fn child_environment(workspace: &Path) -> Vec<(String, String)> {
    const FORWARD: &[&str] = &["PATH", "HOME", "LANG", "LC_ALL", "TERM", "USER", "LBX_RUN_LABEL"];

    let mut environment = Vec::new();
    for name in FORWARD {
        if let Some(value) = std::env::var_os(name) {
            environment.push((name.to_string(), value.to_string_lossy().into_owned()));
        }
    }
    let tmp = workspace.join("tmp");
    environment.push(("TMPDIR".to_string(), tmp.to_string_lossy().into_owned()));
    environment
}

fn prepare_tmpdir(workspace: &Path) -> Result<(), String> {
    let tmp = workspace.join("tmp");
    std::fs::create_dir_all(&tmp).map_err(|err| format!("{}: {err}", tmp.display()))?;
    let resolved = tmp
        .canonicalize()
        .map_err(|err| format!("{}: {err}", tmp.display()))?;
    if !resolved.starts_with(workspace) {
        return Err(format!("{} resolves outside the workspace", tmp.display()));
    }
    Ok(())
}

fn main() {
    let args = match parse() {
        Ok(args) => args,
        Err(err) => {
            eprintln!("sandbox-nono: {err}\n\n{USAGE_TEXT}");
            std::process::exit(USAGE);
        }
    };

    if args.command.is_empty() {
        eprintln!("sandbox-nono: no command after --\n\n{USAGE_TEXT}");
        std::process::exit(USAGE);
    }
    if !Path::new(&args.command[0]).is_absolute() {
        eprintln!("sandbox-nono: {} is not an absolute path", args.command[0]);
        std::process::exit(USAGE);
    }

    let workspace = match grant(&args.workspace, None) {
        Ok(workspace) => workspace,
        Err(err) => {
            eprintln!("sandbox-nono: workspace: {err}");
            std::process::exit(USAGE);
        }
    };

    let mut capabilities = CapabilitySet::new();
    for path in BASELINE_READ {
        // nono rejects paths that do not exist, so the baseline is filtered
        // against the host instead of being a constant. Every skip is reported.
        if !Path::new(path).exists() {
            eprintln!("sandbox-nono: baseline skipped (absent): {path}");
            continue;
        }
        capabilities = match capabilities.allow_path(path, AccessMode::Read) {
            Ok(capabilities) => capabilities,
            Err(err) => {
                eprintln!("sandbox-nono: baseline {path}: {err}");
                std::process::exit(USAGE);
            }
        };
    }
    for path in &args.reads {
        let path = match grant(path, Some(&workspace)) {
            Ok(path) => path,
            Err(err) => {
                eprintln!("sandbox-nono: read: {err}");
                std::process::exit(USAGE);
            }
        };
        capabilities = match capabilities.allow_path(&path, AccessMode::Read) {
            Ok(capabilities) => capabilities,
            Err(err) => {
                eprintln!("sandbox-nono: read {}: {err}", path.display());
                std::process::exit(USAGE);
            }
        };
    }
    capabilities = match capabilities.allow_path(&workspace, AccessMode::ReadWrite) {
        Ok(capabilities) => capabilities,
        Err(err) => {
            eprintln!("sandbox-nono: workspace {}: {err}", workspace.display());
            std::process::exit(USAGE);
        }
    };
    for path in &args.writes {
        let path = match grant(path, Some(&workspace)) {
            Ok(path) => path,
            Err(err) => {
                eprintln!("sandbox-nono: write: {err}");
                std::process::exit(USAGE);
            }
        };
        capabilities = match capabilities.allow_path(&path, AccessMode::ReadWrite) {
            Ok(capabilities) => capabilities,
            Err(err) => {
                eprintln!("sandbox-nono: write {}: {err}", path.display());
                std::process::exit(USAGE);
            }
        };
    }

    capabilities = capabilities.block_network();

    // Gap: nono's macOS profile does not grant writes to /dev/null, so every
    // shell redirection fails. Only a raw rule closes it.
    capabilities =
        match capabilities.platform_rule("(allow file-write* (literal \"/dev/null\"))") {
            Ok(capabilities) => capabilities,
            Err(err) => {
                eprintln!("sandbox-nono: /dev/null rule rejected: {err}");
                std::process::exit(FAIL_CLOSED);
            }
        };

    // Gap: macOS has no per-host grant in the capability model
    // (`allow_tcp_connect` is Linux-only), so a connect grant needs SBPL too.
    // The target is validated by `connect()` and re-emitted from its parts.
    for target in &args.connects {
        let target = match connect(target) {
            Ok(target) => target,
            Err(err) => {
                eprintln!("sandbox-nono: connect: {err}");
                std::process::exit(USAGE);
            }
        };
        let rule = format!("(allow network-outbound (remote ip \"{target}\"))");
        capabilities = match capabilities.platform_rule(rule) {
            Ok(capabilities) => capabilities,
            Err(err) => {
                eprintln!("sandbox-nono: connect {target}: {err}");
                std::process::exit(FAIL_CLOSED);
            }
        };
    }

    // Gap 7: nono owns the profile, so the deny-default message cannot be set.
    // The flag is accepted for CLI parity and the omission is reported rather
    // than quietly ignored.
    if args.tag.is_empty() || args.tag.len() > 64
        || !args
        .tag
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        eprintln!("sandbox-nono: {} is not a valid tag", args.tag);
        std::process::exit(USAGE);
    }
    if args.tag != "litterbox" {
        eprintln!("sandbox-nono: --tag {} not applied: nono owns the profile (gap 7)", args.tag);
    }

    // Gap 8: the capability model has no credential invariants, so a caller
    // grant that overlaps a credential directory would expose it. The custom
    // helper emits these as first-class rules; here they are raw SBPL.
    let home = match std::env::var_os("HOME").map(PathBuf::from) {
        Some(home) if home.is_absolute() => match home.canonicalize() {
            Ok(home) => home,
            Err(err) => {
                eprintln!("sandbox-nono: HOME {}: {err}", home.display());
                std::process::exit(FAIL_CLOSED);
            }
        },
        _ => {
            eprintln!("sandbox-nono: HOME must be set and absolute");
            std::process::exit(FAIL_CLOSED);
        }
    };
    {
        for relative in CREDENTIAL_DENY {
            let path = home.join(relative);
            let path = path.canonicalize().unwrap_or(path);
            let quoted = path
                .to_string_lossy()
                .chars()
                .flat_map(|c| match c {
                    '"' => vec!['\\', '"'],
                    '\\' => vec!['\\', '\\'],
                    c if (c as u32) < 0x20 => format!("\\x{:02x}", c as u32).chars().collect(),
                    c => vec![c],
                })
                .collect::<String>();
            for operation in ["file-read*", "file-write*", "file-write-unlink"] {
                let rule = format!("(deny {operation} (subpath \"{quoted}\"))");
                capabilities = match capabilities.platform_rule(rule) {
                    Ok(next) => next,
                    Err(err) => {
                        eprintln!("sandbox-nono: credential deny rejected: {err}");
                        std::process::exit(FAIL_CLOSED);
                    }
                };
            }
        }
    }

    if let Err(err) = prepare_tmpdir(&workspace) {
        eprintln!("sandbox-nono: TMPDIR: {err}");
        std::process::exit(FAIL_CLOSED);
    }

    if unsafe { setsid() } == -1 && unsafe { getpgrp() } != unsafe { getpid() } {
        eprintln!(
            "sandbox-nono: setsid failed: {}",
            std::io::Error::last_os_error()
        );
        std::process::exit(FAIL_CLOSED);
    }
    if unsafe { getsid(0) } != unsafe { getpid() } {
        eprintln!("sandbox-nono: warning: no new session; job-control signals cross the sandbox boundary");
    }

    if let Err(err) = Sandbox::apply_auto(&capabilities) {
        eprintln!("sandbox-nono: apply failed: {err}");
        std::process::exit(FAIL_CLOSED);
    }

    let environment = child_environment(&workspace);
    // The profile cannot revoke an open descriptor; close everything above
    // stdio before handing control to the command.
    // closefrom() is not in this SDK's libSystem; bound the sweep instead.
    unsafe {
        for fd in 3..1024 {
            close(fd);
        }
    }

    let error = Command::new(&args.command[0])
        .args(&args.command[1..])
        .env_clear()
        .envs(environment)
        .exec();
    eprintln!("sandbox-nono: exec failed: {error}");
    std::process::exit(EXEC_FAILED);
}
