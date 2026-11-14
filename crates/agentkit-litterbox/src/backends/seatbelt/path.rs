//! Path rules the machine-owned section is held to.
//!
//! A configured path is absolute or starts with `~/`. It is normalised
//! lexically so two spellings of one path compare equal. The real home comes
//! from the password database, never from `HOME`.

use std::ffi::{CStr, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use crate::sandbox::contract::Error;

/// Resolve the effective user's home from the password database.
pub(crate) fn real_home() -> Result<PathBuf, Error> {
    let uid = unsafe { libc::getuid() };
    let mut buffer: Vec<libc::c_char> = vec![0; 1024];

    loop {
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: the buffer and entry outlive the call, and `result` points at
        // the caller-provided entry when the lookup succeeds.
        let code = unsafe {
            libc::getpwuid_r(
                uid,
                &mut entry,
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut result,
            )
        };
        if code == 0 {
            if result.is_null() || entry.pw_dir.is_null() {
                return Err(Error::Misconfigured {
                    message: "cannot resolve the home directory from the password database"
                        .to_string(),
                });
            }
            // SAFETY: a successful lookup returned a NUL-terminated pw_dir.
            let bytes = unsafe { CStr::from_ptr(entry.pw_dir) }.to_bytes();
            let home = PathBuf::from(OsStr::from_bytes(bytes));
            if home.as_os_str().is_empty() {
                return Err(Error::Misconfigured {
                    message: "the password database holds an empty home directory".to_string(),
                });
            }
            return Ok(home);
        }
        if code == libc::ERANGE {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        return Err(Error::Misconfigured {
            message: format!(
                "cannot resolve the home directory from the password database (error {code})"
            ),
        });
    }
}

/// Parse one configured path under `key`.
pub(crate) fn parse(value: &str, home: &Path, key: &str) -> Result<PathBuf, Error> {
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid(key, value, "is empty"));
    }
    if value.contains('\0') {
        return Err(invalid(key, value, "contains a NUL byte"));
    }

    let expanded = if value == "~" {
        home.to_path_buf()
    } else if let Some(rest) = value.strip_prefix("~/") {
        home.join(rest)
    } else if value.starts_with('~') {
        return Err(invalid(key, value, "must use ~/ for a home path"));
    } else {
        PathBuf::from(value)
    };

    if !expanded.is_absolute() {
        return Err(invalid(key, value, "must be absolute or start with ~/"));
    }

    let normalised = normalise(&expanded);
    if normalised == Path::new("/") {
        return Err(invalid(key, value, "must not be the root path"));
    }
    Ok(normalised)
}

/// Remove `.` and resolve `..` lexically.
pub(crate) fn normalise(path: &Path) -> PathBuf {
    let mut normalised = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                normalised.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                normalised.pop();
            }
            Component::Normal(part) => normalised.push(part),
        }
    }
    normalised
}

/// Whether a value can name a sandbox in logs and the deny message.
pub(crate) fn validate_tag(value: &str) -> Result<(), String> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        });
    if valid {
        Ok(())
    } else {
        Err(format!(
            "'{value}' is not a valid sandbox tag; use 1 to 64 ASCII letters, digits, '.', '_' or '-'"
        ))
    }
}

fn invalid(key: &str, value: &str, reason: &str) -> Error {
    Error::Misconfigured {
        message: format!("backends.seatbelt.{key} entry '{value}' {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{normalise, parse, real_home, validate_tag};
    use crate::sandbox::contract::Error;

    const HOME: &str = "/Users/tester";

    #[test]
    fn a_home_path_expands_and_an_absolute_path_stays() {
        let home = Path::new(HOME);
        assert_eq!(
            parse("~/src", home, "baseline-reads").expect("expands"),
            PathBuf::from("/Users/tester/src")
        );
        assert_eq!(
            parse("/opt/toolchain", home, "baseline-reads").expect("absolute"),
            PathBuf::from("/opt/toolchain")
        );
    }

    #[test]
    fn a_bare_tilde_is_the_home_and_a_named_tilde_is_refused() {
        let home = Path::new(HOME);
        assert_eq!(
            parse("~", home, "credential-denies").expect("home"),
            PathBuf::from(HOME)
        );
        let error = parse("~other/src", home, "credential-denies").expect_err("refused");
        assert!(error.to_string().contains("~/"), "{error}");
    }

    #[test]
    fn a_relative_or_empty_or_nul_path_is_refused() {
        let home = Path::new(HOME);
        for value in ["relative/path", "", "/tmp/has\0nul"] {
            let error = parse(value, home, "baseline-reads").expect_err("refused");
            match error {
                Error::Misconfigured { message } => {
                    assert!(message.contains("baseline-reads"), "{message}");
                }
                other => panic!("expected Misconfigured, got {other:?}"),
            }
        }
    }

    #[test]
    fn the_root_path_is_refused() {
        let error = parse("/", Path::new(HOME), "baseline-reads").expect_err("refused");
        assert!(error.to_string().contains("root path"), "{error}");
    }

    #[test]
    fn normalisation_removes_dot_and_resolves_parent() {
        assert_eq!(
            normalise(Path::new("/opt/./tool/../chain")),
            PathBuf::from("/opt/chain")
        );
        assert_eq!(normalise(Path::new("/a/b/../..")), PathBuf::from("/"));
    }

    #[test]
    fn a_tag_is_ascii_and_bounded() {
        assert!(validate_tag("demo-1.2_x").is_ok());
        for value in ["", "with space", "with/slash", "ümlaut"] {
            assert!(validate_tag(value).is_err(), "{value}");
        }
        assert!(validate_tag(&"a".repeat(65)).is_err());
    }

    #[test]
    fn the_real_home_is_absolute() {
        let home = real_home().expect("this host has a home");
        assert!(home.is_absolute(), "{}", home.display());
    }
}
