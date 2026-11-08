//! The command that keeps a sandbox alive and the paths it needs.
//!
//! This is the mechanism's own requirement, not the sandbox's userland: bwrap
//! has to run something for as long as the sandbox exists, and that something
//! is a shell that execs one blocking utility. What a workload can run is
//! whatever its provisioning steps contribute.

use std::path::{Path, PathBuf};

use super::BACKEND;
use super::host::Host;
use crate::sandbox::contract::Error;

/// The shell the holder command is evaluated with.
const SHELL: &str = "sh";

/// The utility that blocks for the life of the sandbox.
const SLEEP: &str = "sleep";

/// The mode bits that make the loader ignore a loader trace.
const SET_ID: u32 = 0o6000;

/// The command that keeps a sandbox alive and the paths it needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// The shell that runs the holder command.
    pub shell: PathBuf,
    /// The paths to bind: the shell, the utility and the interpreter and
    /// libraries the host's loader resolves for both.
    pub binds: Vec<PathBuf>,
    /// The directory list the shell searches for the utility, which is not the
    /// sandbox's `PATH`.
    pub path: String,
}

impl Command {
    /// The shell arguments that hold the sandbox.
    ///
    /// The shell looks the utility up by name, which is what a multicall binary
    /// needs to pick its behaviour, and the assignment keeps that search out of
    /// the sandbox's own environment.
    pub fn arguments(&self) -> [String; 2] {
        [
            "-c".to_string(),
            format!("PATH={} exec {SLEEP} infinity", self.path),
        ]
    }
}

/// Resolve the holder command and validate the binaries it runs.
pub fn command(host: &dyn Host) -> Result<Command, Error> {
    let shell = trusted(host, SHELL)?;
    let sleep = trusted(host, SLEEP)?;
    let mut binds = Vec::new();
    let mut directories: Vec<String> = Vec::new();
    for path in [&shell, &sleep] {
        if !binds.contains(path) {
            binds.push(path.clone());
        }
        for dependency in runtime(path, host.ownership(path).map_or(0, |(_, mode)| mode)) {
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
        shell,
        binds,
        path: directories.join(":"),
    })
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
pub fn check(host: &dyn Host) -> crate::sandbox::report::Check {
    use crate::sandbox::report::{Check, State};
    let primitive = "the holder command".to_string();
    match command(host) {
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{command, parse_trace};
    use crate::backends::bubblewrap::host::FakeHost;

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
        let command = command(&holder_host()).expect("command");
        assert_eq!(command.shell, PathBuf::from("/usr/bin/sh"));
        assert!(command.binds.contains(&PathBuf::from("/usr/bin/sh")));
        assert!(command.binds.contains(&PathBuf::from("/usr/bin/sleep")));
        assert_eq!(
            command.arguments(),
            [
                "-c".to_string(),
                "PATH=/usr/bin exec sleep infinity".to_string()
            ]
        );
    }

    #[test]
    fn a_holder_binary_outside_the_host_is_unavailable() {
        let mut host = holder_host();
        host.binaries.remove("sleep");
        let error = command(&host).expect_err("refused");
        assert!(error.to_string().contains("sleep"), "{error}");
    }

    #[test]
    fn a_user_owned_holder_binary_is_refused() {
        let mut host = holder_host();
        host.ownership.insert(PathBuf::from("/usr/bin/sh"), (1000, 0o755));
        let error = command(&host).expect_err("refused");
        assert!(error.to_string().contains("root-owned"), "{error}");
    }
}
