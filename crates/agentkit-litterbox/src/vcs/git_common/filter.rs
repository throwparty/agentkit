//! The snapshot filter: what a snapshot carries, decided like `git add -A`.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use git2::Repository;

use crate::domain::{SandboxError, ScmError};

/// What a snapshot tracks, decided the way `git add -A` decides it: a path the
/// snapshot's parent already records is tracked, and the sandbox's ignore rules
/// apply only to everything else.
pub struct SnapshotFilter {
    /// The checkout libgit2 evaluates rules against. Kept alive because it owns
    /// the directory the sandbox's rule files are mirrored into.
    _rules: tempfile::TempDir,
    checkout: Mutex<Repository>,
    tracked: HashSet<PathBuf>,
    tracked_dirs: HashSet<PathBuf>,
}

impl SnapshotFilter {
    /// Build a filter from the paths the snapshot's parent records.
    pub(crate) fn open(
        rules_root: &Path,
        tracked: HashSet<PathBuf>,
        tracked_dirs: HashSet<PathBuf>,
    ) -> Result<Self, SandboxError> {
        let rules = tempfile::tempdir().map_err(SandboxError::Io)?;
        let checkout = Repository::init(rules.path())
            .map_err(|source| SandboxError::Scm(ScmError::Open { source }))?;

        // libgit2 answers ignore questions about a checkout, so the sandbox's
        // own ignore files are mirrored into this one at the same paths: a
        // nested rule keeps meaning what it means inside the sandbox.
        mirror_ignore_files(rules_root, rules.path())?;

        // A repository's own exclude file applies from its root.
        if let Ok(contents) = fs::read(rules_root.join(".git").join("info").join("exclude")) {
            let target = checkout.path().join("info").join("exclude");
            fs::write(target, contents).map_err(SandboxError::Io)?;
        }

        Ok(Self {
            _rules: rules,
            checkout: Mutex::new(checkout),
            tracked,
            tracked_dirs,
        })
    }

    pub(crate) fn skips_file(&self, relative: &Path) -> Result<bool, SandboxError> {
        if self.tracked.contains(relative) {
            return Ok(false);
        }
        self.ignored(relative)
    }

    /// A directory is skipped only when the snapshot's parent records nothing
    /// beneath it.
    pub(crate) fn skips_dir(&self, relative: &Path) -> Result<bool, SandboxError> {
        if self.tracked_dirs.contains(relative) {
            return Ok(false);
        }
        // The staging tree is not the checkout, so libgit2 cannot stat the
        // directory to learn it is one; the trailing separator says so.
        let mut probe = relative.as_os_str().to_owned();
        probe.push("/");
        self.ignored(Path::new(&probe))
    }

    fn ignored(&self, relative: &Path) -> Result<bool, SandboxError> {
        let checkout = self
            .checkout
            .lock()
            .map_err(|_| SandboxError::Config("Mutex poisoned".to_string()))?;
        checkout
            .status_should_ignore(relative)
            .map_err(|source| SandboxError::Scm(ScmError::Status { source }))
    }
}

/// Copy a tree's ignore files into a checkout, leaving everything else behind.
///
/// `from` is a tree the sandbox produced; `to` is the scratch checkout libgit2
/// reads rules from. Only the rule files are needed, so nothing else is copied
/// and no rule changes meaning: `.gitignore` governs the directory it sits in.
fn mirror_ignore_files(from: &Path, to: &Path) -> Result<(), SandboxError> {
    let entries = match fs::read_dir(from) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(SandboxError::Io(error)),
    };

    for entry in entries {
        let entry = entry.map_err(SandboxError::Io)?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        // `file_type` does not follow a link, so a linked directory is left
        // alone rather than walked in circles.
        if entry.file_type().map_err(SandboxError::Io)?.is_dir() {
            mirror_ignore_files(&entry.path(), &to.join(&name))?;
        } else if name == ".gitignore" {
            fs::create_dir_all(to).map_err(SandboxError::Io)?;
            fs::copy(entry.path(), to.join(&name)).map_err(SandboxError::Io)?;
        }
    }

    Ok(())
}

impl crate::sandbox::contract::SnapshotSelector for SnapshotFilter {
    fn selects(
        &self,
        relative: &Path,
        is_dir: bool,
    ) -> Result<bool, crate::sandbox::contract::Error> {
        let skips = if is_dir {
            self.skips_dir(relative)
        } else {
            self.skips_file(relative)
        };
        skips
            .map(|skips| !skips)
            .map_err(|error| crate::sandbox::contract::Error::failed(error.to_string()))
    }
}
