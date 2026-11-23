//! The subprocess driver: run `jj` and `git` and read their facts.
//!
//! jj 0.47 is already installed; `jj-lib` has no stable API, so the backend
//! speaks the command line. Facts come from templates and `rev-parse`, never
//! from parsing a human display.

#![allow(dead_code)] // wired into the operations in T-008

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::vcs::Error;

/// The oldest jj this backend speaks to.
const MINIMUM: (u32, u32) = (0, 40);

fn run(program: &str, cwd: &Path, args: &[&str]) -> Result<Output, Error> {
    Command::new(program)
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(|error| Error::Backend {
            operation: format!("run {program}"),
            message: format!("could not run {program}: {error}"),
        })
}

fn checked(operation: &str, output: &Output) -> Result<(), Error> {
    if output.status.success() {
        return Ok(());
    }
    Err(Error::Backend {
        operation: operation.to_string(),
        message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

fn text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Run a jj command and return its trimmed stdout.
pub fn jj(cwd: &Path, args: &[&str]) -> Result<String, Error> {
    let output = run("jj", cwd, args)?;
    checked(&format!("jj {}", args.join(" ")), &output)?;
    Ok(text(&output))
}

/// Run a jj command and discard its stdout.
#[allow(dead_code)] // wired into the operations in T-008
pub fn jj_ok(cwd: &Path, args: &[&str]) -> Result<(), Error> {
    let output = run("jj", cwd, args)?;
    checked(&format!("jj {}", args.join(" ")), &output)
}

/// Run a git command and return its trimmed stdout.
pub fn git(cwd: &Path, args: &[&str]) -> Result<String, Error> {
    let output = run("git", cwd, args)?;
    checked(&format!("git {}", args.join(" ")), &output)?;
    Ok(text(&output))
}

/// The commit id a revset resolves to.
pub fn commit_id(cwd: &Path, revset: &str) -> Result<String, Error> {
    jj(cwd, &["log", "--no-graph", "-r", revset, "-T", "commit_id"])
}

/// The tree id of a revision, through the clone's Git backend.
pub fn tree_id(cwd: &Path, revision: &str) -> Result<String, Error> {
    git(cwd, &["rev-parse", &format!("{revision}^{{tree}}")])
}

/// The conflicted paths a revision carries.
///
/// jj exits non-zero and says "No conflicts found" when the revision is clean,
/// which is not an error.
pub fn conflicts(cwd: &Path, revision: &str) -> Result<Vec<PathBuf>, Error> {
    let output = run("jj", cwd, &["resolve", "--list", "-r", revision])?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No conflicts found") {
            return Ok(Vec::new());
        }
        return Err(Error::Backend {
            operation: format!("jj resolve --list -r {revision}"),
            message: stderr.trim().to_string(),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut paths = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(path) = line.split_whitespace().next() {
            paths.push(PathBuf::from(path));
        }
    }
    Ok(paths)
}

fn parse_version(text: &str) -> Option<(u32, u32)> {
    let raw = text.split_whitespace().last()?;
    let mut parts = raw.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    Some((major, minor))
}

/// Check the `jj` on PATH is present and new enough.
pub fn probe(cwd: &Path) -> Result<(), Error> {
    let output = run("jj", cwd, &["--version"])?;
    if !output.status.success() {
        return Err(Error::Backend {
            operation: "probe jj".to_string(),
            message: "the jj binary is not usable".to_string(),
        });
    }
    let version = text(&output);
    match parse_version(&version) {
        Some(found) if found >= MINIMUM => Ok(()),
        Some(found) => Err(Error::Backend {
            operation: "probe jj".to_string(),
            message: format!(
                "jj {}.{} is older than the required {}.{}",
                found.0, found.1, MINIMUM.0, MINIMUM.1
            ),
        }),
        None => Err(Error::Backend {
            operation: "probe jj".to_string(),
            message: format!("could not read the jj version from '{version}'"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .args(args)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    }

    fn repository() -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        git(dir.path(), &["init", "-q"]);
        std::fs::write(dir.path().join("README.md"), "hello").expect("write");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-qm", "init"]);
        dir
    }

    #[test]
    fn facts_come_from_the_clone() {
        let host = repository();
        let data = TempDir::new().expect("data");
        let root = data.path().join("project");
        super::super::clone::ensure(&root, host.path()).expect("clone");

        let base = super::commit_id(&root, "@-").expect("commit id");
        assert_eq!(base.len(), 40, "{base}");

        let tree = super::tree_id(&root, &base).expect("tree id");
        assert_eq!(tree.len(), 40, "{tree}");

        assert!(
            super::conflicts(&root, &base).expect("clean").is_empty(),
            "a clean revision has no conflicted paths"
        );

        super::probe(&root).expect("the installed jj is new enough");
    }

    #[test]
    fn the_version_parses() {
        assert_eq!(super::parse_version("jj 0.47.1"), Some((0, 47)));
        assert_eq!(super::parse_version("jj 1.2.0"), Some((1, 2)));
        assert_eq!(super::parse_version("nonsense"), None);
    }
}
