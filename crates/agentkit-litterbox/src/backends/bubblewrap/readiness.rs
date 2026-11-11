//! What this host must provide before a sandbox can be created.

use std::path::Path;
use std::process::Command;

use super::holder;
use super::host::Host;
use crate::sandbox::report::{Check, Fallback, State};

/// The oldest bubblewrap the renderer supports.
pub const FLOOR: (u64, u64, u64) = (0, 8, 0);

/// Every check, in the order a person reads them.
pub fn checks(host: &dyn Host, shell: &[String]) -> Vec<Check> {
    vec![
        root_check(host),
        bwrap_check(host),
        userns_check(),
        cgroup_check(host),
        holder::check(host, shell),
    ]
}

fn root_check(host: &dyn Host) -> Check {
    if host.effective_uid() == 0 {
        Check {
            primitive: "an unprivileged caller".to_string(),
            state: State::Unavailable,
            reason: Some(
                "litterbox is running as root, and the sandbox relies on the same-uid model"
                    .to_string(),
            ),
            remedy: Some("run it as the user who owns the sandbox".to_string()),
            fallback: None,
        }
    } else {
        Check {
            primitive: "an unprivileged caller".to_string(),
            state: State::Available,
            reason: None,
            remedy: None,
            fallback: None,
        }
    }
}

fn bwrap_check(host: &dyn Host) -> Check {
    let primitive = "bubblewrap".to_string();
    let Some(path) = host.resolve("bwrap") else {
        return Check {
            primitive,
            state: State::Unavailable,
            reason: Some("bwrap does not resolve on the host".to_string()),
            remedy: Some("install bubblewrap or add it to PATH".to_string()),
            fallback: None,
        };
    };
    if let Err(reason) = trusted(host, &path) {
        return Check {
            primitive,
            state: State::Unavailable,
            reason: Some(reason),
            remedy: Some("use a bwrap installed by the package manager".to_string()),
            fallback: None,
        };
    }
    match version(&path) {
        Some(found) if found >= FLOOR => Check {
            primitive,
            state: State::Available,
            reason: None,
            remedy: None,
            fallback: None,
        },
        Some(found) => Check {
            primitive,
            state: State::Unavailable,
            reason: Some(format!("bwrap {found:?} is older than {FLOOR:?}")),
            remedy: Some("upgrade bubblewrap".to_string()),
            fallback: None,
        },
        None => Check {
            primitive,
            state: State::Unavailable,
            reason: Some("bwrap did not report a version".to_string()),
            remedy: Some("reinstall bubblewrap".to_string()),
            fallback: None,
        },
    }
}

fn trusted(host: &dyn Host, path: &Path) -> Result<(), String> {
    let Some((uid, mode)) = host.ownership(path) else {
        return Err(format!("{} cannot be inspected", path.display()));
    };
    if uid != 0 {
        return Err(format!("{} is not owned by root", path.display()));
    }
    if mode & 0o022 != 0 {
        return Err(format!(
            "{} is writable by group or others, so the sandbox will not run it",
            path.display()
        ));
    }
    Ok(())
}

fn version(path: &Path) -> Option<(u64, u64, u64)> {
    let output = Command::new(path).arg("--version").output().ok()?;
    parse_version(&String::from_utf8_lossy(&output.stdout))
}

/// Parse the version out of `bwrap --version`.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let token = text.split_whitespace().find(|token| {
        token
            .trim_start_matches('v')
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_digit())
    })?;
    let mut parts = token.trim_start_matches('v').split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts
        .next()
        .unwrap_or("0")
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(0);
    Some((major, minor, patch))
}

fn userns_check() -> Check {
    let primitive = "unprivileged user namespaces".to_string();
    let restricted = read_sysctl("/proc/sys/kernel/unprivileged_userns_clone")
        .is_some_and(|value| value.trim() == "0")
        || read_sysctl("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")
            .is_some_and(|value| value.trim() == "1");
    if restricted {
        return Check {
            primitive,
            state: State::Unavailable,
            reason: Some("the host policy blocks unprivileged user namespaces".to_string()),
            remedy: Some(
                "allow unprivileged user namespaces in the kernel and AppArmor policy".to_string(),
            ),
            fallback: None,
        };
    }
    Check {
        primitive,
        state: State::Available,
        reason: None,
        remedy: None,
        fallback: None,
    }
}

fn read_sysctl(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn cgroup_check(host: &dyn Host) -> Check {
    let primitive = "a cgroup v2 user scope".to_string();
    let usable = host.user_scope() && std::fs::metadata("/sys/fs/cgroup/cgroup.controllers").is_ok();
    if usable {
        return Check {
            primitive,
            state: State::Available,
            reason: None,
            remedy: None,
            fallback: None,
        };
    }
    Check {
        primitive,
        state: State::Degraded,
        reason: Some(
            "the host has no usable cgroup v2 user scope, so pause cannot be atomic and nothing \
             bounds memory or tasks"
                .to_string(),
        ),
        remedy: Some(
            "enable cgroup v2 with delegation, or set accept-degraded = true to create anyway"
                .to_string(),
        ),
        fallback: Some(Fallback {
            name: "stop and rescan the sandbox pid namespace".to_string(),
            does_not_bound: "memory and task count, which a same-uid workload can evade"
                .to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{FLOOR, parse_version, trusted};
    use crate::backends::bubblewrap::host::FakeHost;
    use std::path::Path;

    #[test]
    fn a_version_is_parsed() {
        assert_eq!(parse_version("bubblewrap 0.12.0"), Some((0, 12, 0)));
        assert_eq!(parse_version("bubblewrap 0.8.0"), Some(FLOOR));
        assert_eq!(parse_version("bubblewrap 0.11.0-dev"), Some((0, 11, 0)));
        assert_eq!(parse_version("bubblewrap v0.12.0"), Some((0, 12, 0)));
        assert_eq!(parse_version("no version here"), None);
    }

    #[test]
    fn a_trusted_binary_is_accepted() {
        let mut host = FakeHost::default();
        let path = std::path::PathBuf::from("/nix/store/abc/bin/bwrap");
        host.ownership.insert(path.clone(), (0, 0o755));
        assert!(trusted(&host, &path).is_ok());
    }

    #[test]
    fn a_user_owned_binary_is_refused() {
        let mut host = FakeHost::default();
        let path = std::path::PathBuf::from("/home/someone/bwrap");
        host.ownership.insert(path.clone(), (1000, 0o755));
        let error = trusted(&host, &path).expect_err("refused");
        assert!(error.contains("root"), "{error}");
    }

    #[test]
    fn a_group_writable_binary_is_refused() {
        let mut host = FakeHost::default();
        let path = std::path::PathBuf::from("/nix/store/abc/bin/bwrap");
        host.ownership.insert(path.clone(), (0, 0o775));
        let error = trusted(&host, &path).expect_err("refused");
        assert!(error.contains("writable"), "{error}");
    }

    #[test]
    fn a_missing_binary_cannot_be_trusted() {
        let host = FakeHost::default();
        assert!(trusted(&host, Path::new("/nix/store/abc/bin/bwrap")).is_err());
    }
}
