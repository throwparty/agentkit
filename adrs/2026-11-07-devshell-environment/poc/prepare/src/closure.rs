//! The store closure, turned into read-only mounts.
//!
//! Every path a mount names must be a store path: the closure rule is the
//! contract's, and a project's own output cannot widen it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::failure::Failure;
use crate::util::{Caller, argv, run};

/// Where a store path lives.
const STORE: &str = "/nix/store";

/// The paths a sandbox sees of the store: the closure, or the whole store.
pub fn closure(
    nix: &Path,
    caller: &Caller,
    output_path: &str,
    whole: bool,
) -> Result<Vec<PathBuf>, Failure> {
    if whole {
        return Ok(vec![PathBuf::from(STORE)]);
    }
    let listed = run(nix, &argv(&["path-info", "-r", output_path]), caller, None)?;
    if !listed.success() {
        return Err(Failure::unavailable(
            format!("nix path-info -r {output_path}"),
            "the devShell's closure could not be queried",
        )
        .with_output(listed.stderr_excerpt())
        .with_remedy("run nix path-info -r on the host to see why"));
    }
    parse_paths(&String::from_utf8_lossy(&listed.stdout))
}

/// De-duplicate a closure query into an ordered list of store paths.
///
/// A line that is not an absolute path under the store is refused rather than
/// mounted: the view is the closure rule, not whatever the query printed.
pub fn parse_paths(listed: &str) -> Result<Vec<PathBuf>, Failure> {
    let mut seen = BTreeSet::new();
    for line in listed.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let path = PathBuf::from(line);
        if !is_store_path(&path) {
            return Err(Failure::misconfigured(format!(
                "the closure query named '{line}', which is not a path under {STORE}"
            ))
            .with_primitive("nix path-info -r")
            .with_remedy("point the step at a devShell whose output is a store path"));
        }
        seen.insert(path);
    }
    Ok(seen.into_iter().collect())
}

fn is_store_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .parent()
            .map(|parent| parent == Path::new(STORE))
            .unwrap_or(false)
        && !path
            .file_name()
            .map(|name| name.to_string_lossy().is_empty())
            .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_whole_store_view_is_the_store_root_alone() {
        let whole = closure(Path::new("/bin/false"), &fake_caller(), "", true).expect("no query");
        assert_eq!(whole, vec![PathBuf::from("/nix/store")]);
    }

    #[test]
    fn a_closure_is_deduplicated_and_ordered() {
        let listed = "\
/nix/store/bbb-dev
/nix/store/aaa-glibc
/nix/store/bbb-dev

/nix/store/ccc-gcc
";
        let paths = parse_paths(listed).expect("parses");
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/nix/store/aaa-glibc"),
                PathBuf::from("/nix/store/bbb-dev"),
                PathBuf::from("/nix/store/ccc-gcc"),
            ]
        );
        assert_eq!(paths.len(), 3, "the count matches the query's set");
    }

    #[test]
    fn the_count_of_a_recorded_closure_matches_its_paths() {
        // A fixture shaped like a real query: one path per line, one duplicate.
        let mut listed = String::new();
        for index in 0..238 {
            listed.push_str(&format!("/nix/store/{index:04}-path\n"));
        }
        listed.push_str("/nix/store/0000-path\n");
        assert_eq!(parse_paths(&listed).expect("parses").len(), 238);
    }

    #[test]
    fn a_path_outside_the_store_is_refused() {
        let failure = parse_paths("/nix/store/aaa-dev\n/etc/passwd\n").unwrap_err();
        assert_eq!(failure.class, crate::failure::Class::Misconfigured);
        assert!(failure.reason.contains("/etc/passwd"), "{}", failure.reason);
    }

    #[test]
    fn a_relative_path_is_refused() {
        let failure = parse_paths("./target\n").unwrap_err();
        assert!(failure.reason.contains("./target"), "{}", failure.reason);
    }

    #[test]
    fn a_store_direct_child_with_no_name_is_refused() {
        assert!(!is_store_path(Path::new("/nix/store")));
        assert!(!is_store_path(Path::new("/nix/store/")));
        assert!(is_store_path(Path::new("/nix/store/abc-dev")));
    }

    fn fake_caller() -> Caller {
        // The whole-store branch never runs a command, so the caller is unused.
        Caller::new().expect("a caller")
    }
}
