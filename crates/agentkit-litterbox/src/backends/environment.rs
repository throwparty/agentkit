//! How a backend judges a contributed environment value.
//!
//! A value that names a path the sandbox cannot see is either trimmed or
//! withheld. The rule is the same for every mechanism, so it lives here; the
//! path each backend reports as its workspace is the only input that differs.

use std::path::PathBuf;

/// What the ungranted-path rule does with one variable's value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Every absolute element names something the sandbox can see.
    Keep,
    /// Some elements do not, and can be dropped without emptying the value.
    Trim(String),
    /// Nothing would remain.
    Remove,
}

/// Whether a variable holds a `:`-separated list of paths.
///
/// Only these are read as lists. Every other value is a string whose meaning
/// is its own, and splitting it would corrupt it: `https://example.com` is the
/// obvious case, where `//example.com` is not a path at all.
fn is_path_list(name: &str) -> bool {
    const LISTS: [&str; 6] = [
        "PATH",
        "MANPATH",
        "INFOPATH",
        "PYTHONPATH",
        "NODE_PATH",
        "CLASSPATH",
    ];
    LISTS.contains(&name) || name.ends_with("_PATH")
}

/// Whether the sandbox resolves one path-like value.
fn granted_element(value: &str, workspace: &str, granted: &[PathBuf]) -> bool {
    let element = std::path::Path::new(value);
    element == std::path::Path::new(workspace)
        || element.starts_with(workspace)
        || granted
            .iter()
            .any(|path| element == path || element.starts_with(path))
}

/// Judge the value of the named variable.
pub(crate) fn verdict(name: &str, value: &str, workspace: &str, granted: &[PathBuf]) -> Verdict {
    if !is_path_list(name) {
        // A value that is one absolute path the sandbox cannot see names
        // something absent, so the variable goes. Anything else is kept as it
        // was written: the backend does not guess what a variable is for.
        let absent = value.starts_with('/') && !granted_element(value, workspace, granted);
        return if absent {
            Verdict::Remove
        } else {
            Verdict::Keep
        };
    }

    let elements: Vec<&str> = value.split(':').collect();
    let absolute = elements
        .iter()
        .filter(|element| element.starts_with('/'))
        .count();
    if absolute == 0 {
        return Verdict::Keep;
    }
    let kept: Vec<&str> = elements
        .into_iter()
        .filter(|element| !element.starts_with('/') || granted_element(element, workspace, granted))
        .collect();
    if kept.len() == value.split(':').count() {
        return Verdict::Keep;
    }
    if kept.is_empty() {
        return Verdict::Remove;
    }
    Verdict::Trim(kept.join(":"))
}

#[cfg(test)]
mod tests {
    use super::{Verdict, verdict};

    fn granted() -> Vec<std::path::PathBuf> {
        vec!["/nix/store/abc".into(), "/src".into()]
    }

    #[test]
    fn a_path_list_keeps_only_its_granted_elements() {
        assert_eq!(
            verdict(
                "PATH",
                "/nix/store/abc/bin:/run/current-system/sw/bin:/bin",
                "/src",
                &granted()
            ),
            Verdict::Trim("/nix/store/abc/bin".to_string())
        );
    }

    #[test]
    fn a_path_list_with_nothing_granted_is_removed() {
        assert_eq!(
            verdict("PATH", "/run/current-system/sw/bin", "/src", &granted()),
            Verdict::Remove
        );
    }

    #[test]
    fn a_fully_granted_path_list_is_kept_verbatim() {
        assert_eq!(
            verdict("PATH", "/nix/store/abc/bin:/src/bin", "/src", &granted()),
            Verdict::Keep
        );
    }

    #[test]
    fn a_value_that_is_not_a_path_list_is_kept_verbatim() {
        assert_eq!(
            verdict("SOME_URL", "https://example.com/thing", "/src", &granted()),
            Verdict::Keep
        );
    }

    #[test]
    fn a_lone_absent_path_is_removed() {
        assert_eq!(
            verdict("SSL_CERT_FILE", "/etc/ssl/certs/ca.crt", "/src", &granted()),
            Verdict::Remove
        );
    }

    #[test]
    fn a_workspace_path_is_granted() {
        assert_eq!(
            verdict("CARGO_HOME", "/src/.cargo", "/src", &granted()),
            Verdict::Keep
        );
    }
}
