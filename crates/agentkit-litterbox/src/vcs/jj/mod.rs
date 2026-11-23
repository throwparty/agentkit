//! The jj backend.
//!
//! It shells out to the `jj` binary. The clone is a co-located jj repository
//! at `vcs/jj/<slug>`; each sandbox is one jj workspace. It imports `vcs` and
//! `vcs::git_common` and never `vcs::git`.

mod clone;
mod config;
mod ops;

pub use config::JjVcsConfig;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::domain::SandboxError;
use crate::sandbox::contract::{BindingSupport, SnapshotSelector};
use crate::vcs::git_common::IgnoreFiles;
use crate::vcs::{
    Capabilities, Conflict, ConflictModel, ConflictSides, Error, ExportSupport, HistoryPolicy,
    Merge, Prepared, RestoreMode, Revision, TreeId, Vcs, Workspace,
};

/// A jj-backed VCS.
pub struct JjVcs {
    root: PathBuf,
    #[allow(dead_code)]
    slug: String,
    host: PathBuf,
    lock: Mutex<()>,
}

impl JjVcs {
    /// Build the backend for one project.
    pub fn open(project_slug: &str) -> Result<Self, SandboxError> {
        let host = std::env::current_dir().map_err(SandboxError::Io)?;
        ops::probe(&host).map_err(|error| SandboxError::Config(error.to_string()))?;
        Ok(Self {
            root: clone::root(project_slug),
            slug: project_slug.to_string(),
            host,
            lock: Mutex::new(()),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn workspace(&self, sandbox: &str) -> PathBuf {
        clone::workspace(&self.root, sandbox)
    }

    fn write_synced(&self, sandbox: &str, revision: &str) -> Result<(), Error> {
        ops::git(
            &self.root,
            &["update-ref", &synced_ref(sandbox), revision],
        )?;
        Ok(())
    }
}

fn branch_name(sandbox: &str) -> String {
    format!("litterbox/{sandbox}")
}

fn synced_ref(sandbox: &str) -> String {
    format!("refs/litterbox/synced/{sandbox}")
}

fn sandbox_of(workspace: &Workspace) -> &str {
    workspace
        .branch()
        .as_str()
        .strip_prefix("litterbox/")
        .unwrap_or_else(|| workspace.branch().as_str())
}

fn unsupported(operation: &str) -> Error {
    Error::Unsupported {
        operation: operation.to_string(),
        backend: "jj".to_string(),
        primitive: "the jj backend".to_string(),
        reason: "it is not implemented in this change".to_string(),
        remedy: "use the git backend for now".to_string(),
    }
}

/// The tree a directory's contents would produce, through the clone's object
/// database, without committing.
fn tree_from_dir(root: &Path, dir: &Path) -> Result<TreeId, Error> {
    let index = tempfile::NamedTempFile::new().map_err(Error::Io)?;
    let git_dir = root.join(".git");
    let staged = std::process::Command::new("git")
        .current_dir(dir)
        .env("GIT_INDEX_FILE", index.path())
        .arg("--git-dir")
        .arg(&git_dir)
        .arg("--work-tree")
        .arg(dir)
        .args(["add", "-A", "--", "."])
        .output()
        .map_err(Error::Io)?;
    if !staged.status.success() {
        return Err(Error::Backend {
            operation: "stage the workspace".to_string(),
            message: String::from_utf8_lossy(&staged.stderr).trim().to_string(),
        });
    }
    let written = std::process::Command::new("git")
        .env("GIT_INDEX_FILE", index.path())
        .arg("--git-dir")
        .arg(&git_dir)
        .arg("write-tree")
        .output()
        .map_err(Error::Io)?;
    if !written.status.success() {
        return Err(Error::Backend {
            operation: "write the workspace tree".to_string(),
            message: String::from_utf8_lossy(&written.stderr).trim().to_string(),
        });
    }
    Ok(TreeId::new(
        String::from_utf8_lossy(&written.stdout).trim().to_string(),
    ))
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), Error> {
    fn hidden(name: &std::ffi::OsStr) -> bool {
        matches!(name.to_str(), Some(".jj" | ".git" | ".litterbox.lock"))
    }
    fn walk(dir: &Path, rel: &Path, out: &mut Vec<(PathBuf, PathBuf)>) -> Result<(), Error> {
        for entry in std::fs::read_dir(dir).map_err(Error::Io)? {
            let entry = entry.map_err(Error::Io)?;
            let name = entry.file_name();
            if hidden(&name) {
                continue;
            }
            let path = entry.path();
            let child = rel.join(&name);
            if path.is_dir() {
                walk(&path, &child, out)?;
            } else {
                out.push((path, child));
            }
        }
        Ok(())
    }
    fn prune(
        dir: &Path,
        rel: &Path,
        keep: &std::collections::HashSet<PathBuf>,
    ) -> Result<(), Error> {
        for entry in std::fs::read_dir(dir).map_err(Error::Io)? {
            let entry = entry.map_err(Error::Io)?;
            let name = entry.file_name();
            if hidden(&name) {
                continue;
            }
            let path = entry.path();
            let child = rel.join(&name);
            if path.is_dir() {
                prune(&path, &child, keep)?;
                if std::fs::read_dir(&path)
                    .map(|mut entries| entries.next().is_none())
                    .unwrap_or(false)
                {
                    let _ = std::fs::remove_dir(&path);
                }
            } else if !keep.contains(&child) {
                std::fs::remove_file(&path).map_err(Error::Io)?;
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    walk(from, Path::new(""), &mut files)?;
    let keep: std::collections::HashSet<PathBuf> =
        files.iter().map(|(_, rel)| rel.clone()).collect();
    for (source, rel) in &files {
        let target = to.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(Error::Io)?;
        }
        std::fs::copy(source, &target).map_err(Error::Io)?;
    }
    prune(to, Path::new(""), &keep)
}

impl Vcs for JjVcs {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            conflict_model: ConflictModel::Blocking,
            op_log: true,
            bindings: BindingSupport::Both,
            export: ExportSupport::GitExport,
        }
    }

    fn prepare(&self, sandbox: &str) -> Result<Prepared, Error> {
        let _guard = self.lock();
        clone::ensure(&self.root, &self.host)?;
        let workspace = self.workspace(sandbox);
        if workspace.exists() {
            return Err(Error::Exists {
                name: sandbox.to_string(),
            });
        }
        // The clone's working copy sits on the imported head; its parent is the
        // base a sandbox starts from.
        let base = ops::commit_id(&self.root, "@-")?;
        let branch = branch_name(sandbox);
        ops::jj_ok(&self.root, &["bookmark", "set", &branch, "-r", &base])?;
        if let Some(parent) = workspace.parent() {
            std::fs::create_dir_all(parent).map_err(Error::Io)?;
        }
        let path = workspace.to_str().ok_or_else(|| Error::Backend {
            operation: "prepare".to_string(),
            message: "the workspace path is not UTF-8".to_string(),
        })?;
        ops::jj_ok(
            &self.root,
            &["workspace", "add", "--name", sandbox, "-r", &base, path],
        )?;
        self.write_synced(sandbox, &base)?;
        Ok(Prepared {
            workspace: Workspace::from_branch(branch),
            base: Revision::new(base),
        })
    }

    fn masked_paths(&self, _workspace: &Workspace) -> Vec<PathBuf> {
        vec![PathBuf::from(".jj")]
    }

    fn bound_root(&self, workspace: &Workspace) -> Result<PathBuf, Error> {
        Ok(self.workspace(sandbox_of(workspace)))
    }

    fn identity(&self, workspace: &Workspace) -> Result<TreeId, Error> {
        let tip = self.tip(workspace)?;
        self.tree(&tip)
    }

    fn materialise(
        &self,
        _workspace: &Workspace,
        revision: &Revision,
        into: &Path,
    ) -> Result<(), Error> {
        let output = std::process::Command::new("git")
            .current_dir(&self.root)
            .args(["archive", "--format=tar", revision.as_str()])
            .output()
            .map_err(Error::Io)?;
        if !output.status.success() {
            return Err(Error::Backend {
                operation: "materialise".to_string(),
                message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }
        crate::vcs::git_common::archive::unpack(&output.stdout, into).map_err(|error| {
            Error::Backend {
                operation: "materialise".to_string(),
                message: error.to_string(),
            }
        })
    }

    fn snapshot_filter(&self, _rules_root: &Path) -> Result<Arc<dyn SnapshotSelector>, Error> {
        Ok(Arc::new(IgnoreFiles))
    }

    fn ignore_files(&self) -> Arc<dyn SnapshotSelector> {
        Arc::new(IgnoreFiles)
    }

    fn snapshot(
        &self,
        workspace: &Workspace,
        staging: &Path,
        _message: &str,
    ) -> Result<Revision, Error> {
        let _guard = self.lock();
        let sandbox = sandbox_of(workspace);
        let target = self.workspace(sandbox);
        if staging != target {
            copy_tree(staging, &target)?;
        }
        // Any jj command snapshots the working copy into `@`.
        Ok(Revision::new(ops::commit_id(&target, "@")?))
    }

    fn sandboxes(&self) -> Result<Vec<String>, Error> {
        Ok(Vec::new())
    }

    fn delete_branch(&self, _sandbox: &str) -> Result<(), Error> {
        Err(unsupported("delete_branch"))
    }

    fn state_path(&self) -> Option<PathBuf> {
        Some(self.root.clone())
    }

    fn expose(&self, _name: &str) -> Result<bool, Error> {
        Err(unsupported("expose"))
    }

    fn prune(&self) -> Result<(), Error> {
        Ok(())
    }

    fn tip(&self, workspace: &Workspace) -> Result<Revision, Error> {
        let branch = workspace.branch().as_str();
        Ok(Revision::new(ops::git(
            &self.root,
            &["rev-parse", &format!("refs/heads/{branch}")],
        )?))
    }

    fn set_tip(&self, workspace: &Workspace, revision: &Revision) -> Result<(), Error> {
        let branch = workspace.branch().as_str();
        ops::jj_ok(
            &self.root,
            &["bookmark", "set", branch, "-r", revision.as_str()],
        )?;
        Ok(())
    }

    fn synced(&self, workspace: &Workspace) -> Result<Revision, Error> {
        let sandbox = sandbox_of(workspace);
        Ok(Revision::new(ops::git(
            &self.root,
            &["rev-parse", &synced_ref(sandbox)],
        )?))
    }

    fn set_synced(&self, workspace: &Workspace, revision: &Revision) -> Result<(), Error> {
        self.write_synced(sandbox_of(workspace), revision.as_str())
    }

    fn tree(&self, revision: &Revision) -> Result<TreeId, Error> {
        Ok(TreeId::new(ops::tree_id(&self.root, revision.as_str())?))
    }

    fn sandbox_tip(&self, workspace: &Workspace) -> Result<Revision, Error> {
        let target = self.workspace(sandbox_of(workspace));
        Ok(Revision::new(ops::commit_id(&target, "@")?))
    }

    fn set_sandbox_tip(
        &self,
        workspace: &Workspace,
        revision: &Revision,
    ) -> Result<(), Error> {
        let target = self.workspace(sandbox_of(workspace));
        ops::jj_ok(&target, &["new", revision.as_str()])?;
        Ok(())
    }

    fn advance(
        &self,
        workspace: &Workspace,
        revision: &Revision,
        expected: &Revision,
    ) -> Result<(), Error> {
        let _guard = self.lock();
        if &self.tip(workspace)? != expected {
            return Err(Error::Raced);
        }
        self.set_tip(workspace, revision)
    }

    fn observe(&self, workspace: &Workspace, staging: &Path) -> Result<TreeId, Error> {
        let target = self.workspace(sandbox_of(workspace));
        if staging == target {
            // Bound: force the snapshot, then read `@`'s tree.
            let revision = ops::commit_id(&target, "@")?;
            return Ok(TreeId::new(ops::tree_id(&self.root, &revision)?));
        }
        tree_from_dir(&self.root, staging)
    }

    fn is_ancestor(&self, ancestor: &Revision, descendant: &Revision) -> Result<bool, Error> {
        let output = std::process::Command::new("git")
            .current_dir(&self.root)
            .args([
                "merge-base",
                "--is-ancestor",
                ancestor.as_str(),
                descendant.as_str(),
            ])
            .output()
            .map_err(Error::Io)?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(Error::Backend {
                operation: "is_ancestor".to_string(),
                message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            }),
        }
    }

    fn ensure_refs(&self, workspace: &Workspace) -> Result<(), Error> {
        let sandbox = sandbox_of(workspace);
        if ops::git(
            &self.root,
            &["rev-parse", "--verify", &synced_ref(sandbox)],
        )
        .is_err()
        {
            let tip = self.tip(workspace)?;
            self.write_synced(sandbox, tip.as_str())?;
        }
        Ok(())
    }

    fn merge(
        &self,
        _base: &Revision,
        _ours: &Revision,
        _theirs: &Revision,
    ) -> Result<Merge, Error> {
        Err(unsupported("merge"))
    }

    fn conflicts(&self, _revision: &Revision) -> Result<Vec<Conflict>, Error> {
        Err(unsupported("conflicts"))
    }

    fn conflict_sides(&self, _revision: &Revision, _path: &Path) -> Result<ConflictSides, Error> {
        Err(unsupported("conflict_sides"))
    }

    fn resolve(&self, _revision: &Revision, _paths: &[PathBuf]) -> Result<Revision, Error> {
        Err(unsupported("resolve"))
    }

    fn abort(&self, _revision: &Revision) -> Result<(), Error> {
        Err(unsupported("abort"))
    }

    fn history(
        &self,
        _workspace: &Workspace,
        _keep: &HistoryPolicy,
    ) -> Result<Vec<Revision>, Error> {
        Err(unsupported("history"))
    }

    fn restore(
        &self,
        _workspace: &Workspace,
        _revision: &Revision,
        _mode: RestoreMode,
    ) -> Result<(), Error> {
        Err(unsupported("restore"))
    }

    fn export(&self) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(test)]
impl JjVcs {
    /// Build a backend over an explicit clone, for tests without the data dir.
    pub(crate) fn open_at(root: PathBuf, host: PathBuf) -> Result<Self, SandboxError> {
        ops::probe(&host).map_err(|error| SandboxError::Config(error.to_string()))?;
        Ok(Self {
            root,
            slug: "test".to_string(),
            host,
            lock: Mutex::new(()),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use super::JjVcs;
    use crate::vcs::Vcs;

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
    fn a_sandbox_prepares_snapshots_and_advances() {
        // jj needs an identity to snapshot the working copy.
        unsafe {
            std::env::set_var("JJ_USER", "Test");
            std::env::set_var("JJ_EMAIL", "test@example.com");
        }

        let host = repository();
        let data = TempDir::new().expect("data");
        let vcs = JjVcs::open_at(data.path().join("clone"), host.path().to_path_buf())
            .expect("backend");

        let prepared = vcs.prepare("demo").expect("prepare");
        let workspace = prepared.workspace.clone();
        assert_eq!(vcs.tip(&workspace).expect("tip"), prepared.base);
        assert_eq!(vcs.synced(&workspace).expect("synced"), prepared.base);
        assert_eq!(vcs.tree(&prepared.base).expect("tree").as_str().len(), 40);
        assert_eq!(
            vcs.identity(&workspace).expect("identity"),
            vcs.tree(&prepared.base).expect("tree")
        );

        let staging = TempDir::new().expect("staging");
        std::fs::write(staging.path().join("README.md"), "hello").expect("write");
        std::fs::write(staging.path().join("new.txt"), "new").expect("write");
        let snapshot = vcs.snapshot(&workspace, staging.path(), "snap").expect("snapshot");
        assert_ne!(snapshot, prepared.base);
        assert_eq!(vcs.sandbox_tip(&workspace).expect("sandbox tip"), snapshot);

        vcs.advance(&workspace, &snapshot, &prepared.base)
            .expect("advance");
        assert_eq!(vcs.tip(&workspace).expect("tip"), snapshot);
        vcs.set_synced(&workspace, &snapshot).expect("record synced");
        assert_eq!(vcs.synced(&workspace).expect("synced"), snapshot);
    }
}

