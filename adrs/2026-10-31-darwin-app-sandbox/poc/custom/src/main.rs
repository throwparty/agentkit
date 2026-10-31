mod policy;

use std::ffi::{CStr, CString, c_char};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use policy::Policy;

unsafe extern "C" {
    fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> i32;
    fn setsid() -> i32;
    fn getpgrp() -> i32;
    fn getpid() -> i32;
    fn getsid(pid: i32) -> i32;
    fn close(fd: i32) -> i32;
}

/// Exit codes. A supervisor must be able to tell "the command never ran" from
/// "the command ran and failed".
const USAGE: i32 = 64;
const FAIL_CLOSED: i32 = 70;
const EXEC_FAILED: i32 = 126;

const USAGE_TEXT: &str = "\
usage: sandbox-custom --workspace <path> [options] -- <command> [args...]

options:
  --workspace <path>      sandbox root, read-write (required, absolute)
  --read <path>           additional read-only grant (repeatable)
  --write <path>          additional read-write grant (repeatable)
  --connect <host:port>   egress grant (repeatable; localhost or * only)
  --tag <name>            sandbox identifier for denial attribution
  --dump-profile <path>   write the applied profile to a file
  -h, --help              this text

The command runs in its own session and process group: signal the group, not
the pid. The child environment is rebuilt from PATH, HOME, TMPDIR, LANG,
LC_ALL, TERM and USER; everything else is dropped.
";

struct Args {
    workspace: PathBuf,
    reads: Vec<PathBuf>,
    writes: Vec<PathBuf>,
    connects: Vec<String>,
    tag: String,
    dump_profile: Option<PathBuf>,
    command: Vec<String>,
}

fn parse() -> Result<Args, String> {
    let mut workspace = None;
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    let mut connects = Vec::new();
    let mut tag = "litterbox".to_string();
    let mut dump_profile = None;
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
            "--dump-profile" => {
                dump_profile = Some(PathBuf::from(
                    args.next().ok_or("--dump-profile needs a path")?,
                ))
            }
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
        dump_profile,
        command,
    })
}

fn apply(profile: &str) -> Result<(), String> {
    let profile = CString::new(profile).map_err(|_| "profile contained a NUL".to_string())?;
    let mut error: *mut c_char = std::ptr::null_mut();
    let result = unsafe { sandbox_init(profile.as_ptr(), 0, &mut error) };
    if result == 0 {
        return Ok(());
    }
    let message = if error.is_null() {
        "unknown error".to_string()
    } else {
        unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned()
    };
    Err(message)
}

/// Rebuild the child environment rather than inheriting it: credentials in
/// the parent's environment are not covered by path denies. HOME stays real so
/// the credential denies are the only thing standing between the child and
/// them; TMPDIR is redirected into the workspace, which is the only scratch
/// area the profile grants.
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

/// Create the scratch directory the profile grants, and refuse to exec if it
/// cannot be created or resolves outside the workspace.
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
            eprintln!("sandbox-custom: {err}\n\n{USAGE_TEXT}");
            std::process::exit(USAGE);
        }
    };

    if args.command.is_empty() {
        eprintln!("sandbox-custom: no command after --\n\n{USAGE_TEXT}");
        std::process::exit(USAGE);
    }
    if !Path::new(&args.command[0]).is_absolute() {
        eprintln!(
            "sandbox-custom: {} is not an absolute path",
            args.command[0]
        );
        std::process::exit(USAGE);
    }

    let mut policy = match Policy::new(&args.workspace) {
        Ok(policy) => policy,
        Err(err) => {
            eprintln!("sandbox-custom: workspace: {err}");
            std::process::exit(USAGE);
        }
    };
    policy = match policy.tag(&args.tag) {
        Ok(policy) => policy,
        Err(err) => {
            eprintln!("sandbox-custom: tag: {err}");
            std::process::exit(USAGE);
        }
    };
    for path in &args.reads {
        policy = match policy.read(path) {
            Ok(policy) => policy,
            Err(err) => {
                eprintln!("sandbox-custom: read: {err}");
                std::process::exit(USAGE);
            }
        };
    }
    for path in &args.writes {
        policy = match policy.write(path) {
            Ok(policy) => policy,
            Err(err) => {
                eprintln!("sandbox-custom: write: {err}");
                std::process::exit(USAGE);
            }
        };
    }
    for target in &args.connects {
        policy = match policy.connect(target) {
            Ok(policy) => policy,
            Err(err) => {
                eprintln!("sandbox-custom: connect: {err}");
                std::process::exit(USAGE);
            }
        };
    }

    // Attribute denials to the command, not just the sandbox: the deny message
    // is fixed when the profile is rendered, and this is the only hook into it.
    if args.tag == "litterbox" {
        let command = Path::new(&args.command[0])
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "command".to_string());
        let sanitised: String = command
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .take(32)
            .collect();
        policy = match policy.tag(&format!("lbx-{}", sanitised.trim_matches('-'))) {
            Ok(policy) => policy,
            Err(err) => {
                eprintln!("sandbox-custom: tag: {err}");
                std::process::exit(USAGE);
            }
        };
    }

    // Own session and process group, so a supervisor can pause, resume and
    // kill the tree. setsid fails with EPERM when this process is already a
    // group leader — the normal case when the helper is started as a job — so
    // only fail when no session or group of our own exists.
    if unsafe { setsid() } == -1 && unsafe { getpgrp() } != unsafe { getpid() } {
        eprintln!(
            "sandbox-custom: setsid failed: {}",
            std::io::Error::last_os_error()
        );
        std::process::exit(FAIL_CLOSED);
    }

    if let Err(err) = prepare_tmpdir(policy.workspace()) {
        eprintln!("sandbox-custom: TMPDIR: {err}");
        std::process::exit(FAIL_CLOSED);
    }
    if unsafe { getsid(0) } != unsafe { getpid() } {
        eprintln!("sandbox-custom: warning: no new session; job-control signals cross the sandbox boundary");
    }

    let profile = policy.render();
    if let Some(path) = &args.dump_profile {
        // The dump happens before confinement and with the supervisor's
        // privileges: refuse anything the sandboxed agent could have planted,
        // and anything inside the workspace it can write.
        if !path.is_absolute() {
            eprintln!("sandbox-custom: --dump-profile needs an absolute path");
            std::process::exit(USAGE);
        }
        let resolved = match path.parent().and_then(|parent| parent.canonicalize().ok()) {
            Some(parent) => parent.join(path.file_name().unwrap_or_default()),
            None => {
                eprintln!(
                    "sandbox-custom: cannot resolve the parent of {}",
                    path.display()
                );
                std::process::exit(USAGE);
            }
        };
        if resolved.starts_with(policy.workspace()) {
            eprintln!(
                "sandbox-custom: refusing to dump the profile inside the workspace"
            );
            std::process::exit(USAGE);
        }
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&resolved)
            .and_then(|mut file| std::io::Write::write_all(&mut file, profile.as_bytes()));
        if let Err(err) = written {
            eprintln!("sandbox-custom: could not create {}: {err}", resolved.display());
            std::process::exit(USAGE);
        }
    }

    if let Err(err) = apply(&profile) {
        eprintln!("sandbox-custom: sandbox_init failed: {err}");
        std::process::exit(FAIL_CLOSED);
    }

    let environment = child_environment(policy.workspace());
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
    eprintln!("sandbox-custom: exec failed: {error}");
    std::process::exit(EXEC_FAILED);
}

#[cfg(test)]
mod tests {
    use super::apply;

    #[test]
    fn malformed_profile_reports_the_library_message() {
        let error = apply("(version 1)(deny default)(this-is-not-a-rule")
            .expect_err("malformed profile must not apply");
        assert!(
            !error.trim().is_empty(),
            "the library must say something useful"
        );
        assert!(
            !error.contains("unknown error"),
            "the message, not the fallback, must be reported"
        );
    }

    #[test]
    fn nul_in_profile_is_rejected_before_apply() {
        let error = apply("(version 1)\0(deny default)").expect_err("NUL must be rejected");
        assert!(error.contains("NUL"));
    }
}
