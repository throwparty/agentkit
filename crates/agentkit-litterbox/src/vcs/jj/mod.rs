//! The jj backend.
//!
//! It shells out to the `jj` binary. The clone is a co-located jj repository
//! at `vcs/jj/<slug>`; each sandbox is one jj workspace. It imports `vcs` and
//! `vcs::git_common` and never `vcs::git`.

mod clone;
mod config;
mod history;
mod ops;
mod restore;

pub use config::JjVcsConfig;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::domain::SandboxError;
use crate::sandbox::contract::{BindingSupport, SnapshotSelector};
use crate::vcs::git_common::IgnoreFiles;
use crate::vcs::{
    Begun, Capabilities, Conflict, ConflictModel, ConflictSides, Error, ExportSupport, Finished,
    HistoryStep, Merge, Prepared, Report, Restored, Revision, Staged, TreeId, Vcs, Workspace,
};

/// A jj-backed VCS.
/// Held while a jj mutation runs: the in-process mutex plus the cross-process
/// clone lock.
struct Guard<'a> {
    _local: std::sync::MutexGuard<'a, ()>,
    _file: clone::Lock,
}

pub struct JjVcs {
    root: PathBuf,
    #[allow(dead_code)]
    slug: String,
    host: PathBuf,
    lock: Mutex<()>,
    config: Option<String>,
}

impl JjVcs {
    /// Build the backend for one project.
    pub fn open(project_slug: &str, config: Option<String>) -> Result<Self, SandboxError> {
        let host = std::env::current_dir().map_err(SandboxError::Io)?;
        ops::probe(&host).map_err(|error| SandboxError::Config(error.to_string()))?;
        let vcs = Self {
            root: clone::root(project_slug),
            slug: project_slug.to_string(),
            host,
            lock: Mutex::new(()),
            config,
        };
        vcs.apply_config()
            .map_err(|error| SandboxError::Config(error.to_string()))?;
        Ok(vcs)
    }

    /// Apply the per-repo config string the clone includes.
    ///
    /// jj keeps a repository's config under the user's config directory, keyed
    /// by the repository id, so `jj config path --repo` is the only stable way
    /// to find it. The file is replaced rather than appended, so a changed or
    /// removed string clears the previous settings.
    fn apply_config(&self) -> Result<(), Error> {
        if !self.root.join(".jj").is_dir() {
            return Ok(());
        }
        let path = PathBuf::from(ops::jj(&self.root, &["config", "path", "--repo"])?);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(Error::Io)?;
        }
        let temporary = path.with_file_name("config.toml.next");
        std::fs::write(&temporary, self.config.as_deref().unwrap_or("")).map_err(Error::Io)?;
        let text = temporary.to_str().ok_or_else(|| Error::Backend {
            operation: "apply vcs.jj config".to_string(),
            message: "the config path is not UTF-8".to_string(),
        })?;
        ops::jj(
            &self.root,
            &[
                "--ignore-working-copy",
                "--config-file",
                text,
                "config",
                "list",
            ],
        )?;
        std::fs::rename(&temporary, &path).map_err(Error::Io)?;
        Ok(())
    }

    fn lock(&self) -> Result<Guard<'_>, Error> {
        let local = self
            .lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let file = clone::lock(&self.root)?;
        Ok(Guard {
            _local: local,
            _file: file,
        })
    }

    fn workspace(&self, sandbox: &str) -> PathBuf {
        clone::workspace(&self.root, sandbox)
    }

    fn write_synced(&self, sandbox: &str, revision: &str) -> Result<(), Error> {
        ops::git(&self.root, &["update-ref", &synced_ref(sandbox), revision])?;
        Ok(())
    }

    fn write_sandbox(&self, sandbox: &str, revision: &str) -> Result<(), Error> {
        ops::git(
            &self.root,
            &["update-ref", &sandbox_ref(sandbox), revision],
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

/// The saved sandbox revision, kept apart from the workspace's mutable `@`.
fn sandbox_ref(sandbox: &str) -> String {
    format!("refs/litterbox/sandbox/{sandbox}")
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

/// Selects every path in the full-content pass.
struct SelectAll;

impl SnapshotSelector for SelectAll {
    fn selects(
        &self,
        _relative: &Path,
        _is_dir: bool,
    ) -> Result<bool, crate::sandbox::contract::Error> {
        Ok(true)
    }
}

/// The tree a directory's contents would produce, through the clone's object
/// database, without committing.
fn tree_from_dir(root: &Path, dir: &Path) -> Result<TreeId, Error> {
    let index = tempfile::NamedTempFile::new().map_err(Error::Io)?;
    // `git add` rejects a pre-created empty index. It must create the index
    // itself; keep the temporary path so it is removed on drop.
    std::fs::remove_file(index.path()).map_err(Error::Io)?;
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
            let metadata = std::fs::symlink_metadata(&path).map_err(Error::Io)?;
            if metadata.file_type().is_symlink() {
                out.push((path, child));
            } else if metadata.is_dir() {
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
            let metadata = std::fs::symlink_metadata(&path).map_err(Error::Io)?;
            if metadata.is_dir() {
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
        let metadata = std::fs::symlink_metadata(source).map_err(Error::Io)?;
        if metadata.file_type().is_symlink() {
            // A sandbox-controlled link is copied as a link. Never follow it:
            // its target may point anywhere on the host.
            if std::fs::symlink_metadata(&target).is_ok() {
                std::fs::remove_file(&target).map_err(Error::Io)?;
            }
            let link = std::fs::read_link(source).map_err(Error::Io)?;
            std::os::unix::fs::symlink(link, &target).map_err(Error::Io)?;
        } else {
            std::fs::copy(source, &target).map_err(Error::Io)?;
        }
    }
    prune(to, Path::new(""), &keep)
}

fn merge_conflicts(
    root: &Path,
    base: &Revision,
    branch: &Revision,
    sandbox: &Revision,
) -> Result<Vec<PathBuf>, Error> {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .args([
            "merge-tree",
            "--write-tree",
            &format!("--merge-base={}", base.as_str()),
            branch.as_str(),
            sandbox.as_str(),
        ])
        .output()
        .map_err(Error::Io)?;
    match output.status.code() {
        Some(0) => Ok(Vec::new()),
        Some(1) => Ok(parse_conflict_paths(&String::from_utf8_lossy(
            &output.stdout,
        ))),
        _ => Err(Error::Backend {
            operation: "report".to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        }),
    }
}

fn parse_conflict_paths(stdout: &str) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for line in stdout.lines() {
        let Some((meta, path)) = line.split_once('\t') else {
            continue;
        };
        if matches!(meta.split_whitespace().last(), Some("1" | "2" | "3"))
            && !paths.iter().any(|seen| seen == Path::new(path))
        {
            paths.push(PathBuf::from(path));
        }
    }
    paths
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
        let _guard = self.lock()?;
        clone::ensure(&self.root, &self.host)?;
        self.apply_config()?;
        let workspace = self.workspace(sandbox);
        if workspace.exists() {
            return Err(Error::Exists {
                name: sandbox.to_string(),
            });
        }
        // The host checkout is the creation source. The clone is only as fresh
        // as its last import, so resolve the host HEAD and bring its commit in
        // when the clone does not hold it.
        let host_head = ops::git(&self.host, &["rev-parse", "HEAD"])?;
        if ops::git(&self.root, &["cat-file", "-e", &host_head]).is_err() {
            let host_path = self.host.to_str().ok_or_else(|| Error::Backend {
                operation: "prepare".to_string(),
                message: "the host path is not UTF-8".to_string(),
            })?;
            ops::git(&self.root, &["fetch", host_path, &host_head])?;
        }
        let base = host_head;
        let branch = branch_name(sandbox);
        let mut bookmarked = false;
        let mut workspace_added = false;
        let result = (|| -> Result<(), Error> {
            ops::jj_ok(&self.root, &["bookmark", "set", &branch, "-r", &base])?;
            bookmarked = true;
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
            workspace_added = true;
            self.write_synced(sandbox, &base)?;
            self.write_sandbox(sandbox, &base)?;
            Ok(())
        })();
        if let Err(error) = result {
            if workspace_added {
                let _ = ops::jj_ok(&self.root, &["workspace", "forget", sandbox]);
                let _ = std::fs::remove_dir_all(&workspace);
            }
            if bookmarked {
                let _ = ops::jj_ok(&self.root, &["bookmark", "delete", &branch]);
            }
            let _ = ops::git(&self.root, &["update-ref", "-d", &synced_ref(sandbox)]);
            let _ = ops::git(&self.root, &["update-ref", "-d", &sandbox_ref(sandbox)]);
            return Err(error);
        }
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

    fn blob(&self, tree: &TreeId, path: &Path) -> Result<Option<Vec<u8>>, Error> {
        let Some(path_text) = path.to_str() else {
            return Ok(None);
        };
        ops::blob(&self.root, tree.as_str(), path_text)
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

    fn snapshot_filter(
        &self,
        _workspace: &Workspace,
        _rules_root: &Path,
    ) -> Result<Arc<dyn SnapshotSelector>, Error> {
        // The second transfer is the full content pass. `IgnoreFiles` is only
        // for the first pass that fetches the ignore inputs; selecting it here
        // would omit every ordinary tracked file.
        Ok(Arc::new(SelectAll))
    }

    fn ignore_files(&self) -> Arc<dyn SnapshotSelector> {
        Arc::new(IgnoreFiles)
    }

    fn snapshot(
        &self,
        workspace: &Workspace,
        staging: &Path,
        message: &str,
    ) -> Result<Revision, Error> {
        let _guard = self.lock()?;
        let sandbox = sandbox_of(workspace);
        let target = self.workspace(sandbox);
        if staging != target {
            copy_tree(staging, &target)?;
        }
        // Snapshot the working copy into a commit and publish it. `@` is
        // mutable: any later command may rewrite it, so move to a fresh child
        // and record the published commit as the saved baseline.
        ops::jj_ok(&target, &["describe", "-m", message])?;
        let published = ops::commit_id(&target, "@")?;
        ops::jj_ok(&target, &["new"])?;
        self.write_sandbox(sandbox, &published)?;
        Ok(Revision::new(published))
    }

    fn sandboxes(&self) -> Result<Vec<String>, Error> {
        if !self.root.join(".jj").is_dir() {
            return Ok(Vec::new());
        }
        let output = ops::git(
            &self.root,
            &[
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/heads/litterbox/",
            ],
        )?;
        let mut names: Vec<String> = output
            .lines()
            .filter_map(|line| line.trim().strip_prefix("litterbox/"))
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }

    fn delete_branch(&self, sandbox: &str) -> Result<(), Error> {
        let _guard = self.lock()?;
        let branch = branch_name(sandbox);
        let workspace = self.workspace(sandbox);
        let has_branch = ops::git(
            &self.root,
            &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
        )
        .is_ok();
        let has_workspace = workspace.is_dir();
        if !has_branch && !has_workspace {
            return Err(Error::NotFound {
                name: sandbox.to_string(),
            });
        }
        let _ = ops::jj_ok(&self.root, &["workspace", "forget", sandbox]);
        if has_workspace {
            let _ = std::fs::remove_dir_all(&workspace);
        }
        let _ = ops::jj_ok(&self.root, &["bookmark", "delete", &branch]);
        let _ = ops::git(&self.root, &["update-ref", "-d", &synced_ref(sandbox)]);
        let _ = ops::git(&self.root, &["update-ref", "-d", &sandbox_ref(sandbox)]);
        Ok(())
    }

    fn state_path(&self) -> Option<PathBuf> {
        Some(self.root.clone())
    }

    fn expose(&self, name: &str) -> Result<bool, Error> {
        let output = std::process::Command::new("git")
            .current_dir(&self.host)
            .args(["remote", "add", name])
            .arg(&self.root)
            .output()
            .map_err(Error::Io)?;
        if output.status.success() {
            return Ok(true);
        }
        let existing = ops::git(&self.host, &["remote", "get-url", name])?;
        if Path::new(existing.trim()) == self.root {
            return Ok(false);
        }
        Err(Error::Backend {
            operation: "expose".to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }

    fn prune(&self) -> Result<(), Error> {
        if !self.root.join(".jj").is_dir() {
            return Ok(());
        }
        let names = self.sandboxes()?;
        let dir = self.root.join("workspaces");
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !names.contains(&name) {
                    let _ = ops::jj_ok(&self.root, &["workspace", "forget", &name]);
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        if names.is_empty() {
            let _ = std::fs::remove_dir_all(&self.root);
        }
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

    fn deleted_paths(&self, from: &TreeId, to: &TreeId) -> Result<Vec<PathBuf>, Error> {
        let output = ops::git(
            &self.root,
            &[
                "diff",
                "--name-only",
                "--diff-filter=D",
                from.as_str(),
                to.as_str(),
            ],
        )?;
        Ok(output
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(PathBuf::from)
            .collect())
    }

    fn changed_paths(&self, from: &TreeId, to: &TreeId) -> Result<Vec<PathBuf>, Error> {
        let output = ops::git(
            &self.root,
            &[
                "diff",
                "--name-only",
                "--diff-filter=DM",
                from.as_str(),
                to.as_str(),
            ],
        )?;
        Ok(output
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(PathBuf::from)
            .collect())
    }

    fn sandbox_tip(&self, workspace: &Workspace) -> Result<Revision, Error> {
        let sandbox = sandbox_of(workspace);
        // A read of the saved ref never touches the workspace: running jj
        // there would snapshot `@` and change the very revision this answers.
        match ops::git(&self.root, &["rev-parse", &sandbox_ref(sandbox)]) {
            Ok(revision) => Ok(Revision::new(revision)),
            Err(_) => Err(Error::NotFound {
                name: sandbox.to_string(),
            }),
        }
    }

    fn set_sandbox_tip(&self, workspace: &Workspace, revision: &Revision) -> Result<(), Error> {
        let sandbox = sandbox_of(workspace);
        self.write_sandbox(sandbox, revision.as_str())?;
        let target = self.workspace(sandbox);
        if target.is_dir() {
            // Keep `@` a throwaway child, so editing the workspace cannot
            // rewrite the revision that was just recorded.
            ops::jj_ok(&target, &["new", revision.as_str()])?;
        }
        Ok(())
    }

    fn working_copy_tip(&self, workspace: &Workspace) -> Result<Revision, Error> {
        let sandbox = sandbox_of(workspace);
        let target = self.workspace(sandbox);
        if !target.is_dir() {
            return Err(Error::NotFound {
                name: sandbox.to_string(),
            });
        }
        Ok(Revision::new(ops::commit_id(&target, "@")?))
    }

    fn advance(
        &self,
        workspace: &Workspace,
        revision: &Revision,
        expected: &Revision,
    ) -> Result<(), Error> {
        let _guard = self.lock()?;
        let branch = workspace.branch().as_str();
        let reference = format!("refs/heads/{branch}");
        ops::git_update_ref(&self.root, &reference, revision.as_str(), expected.as_str())?;
        ops::jj_ok(&self.root, &["git", "import"])?;
        Ok(())
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
        if ops::git(&self.root, &["rev-parse", "--verify", &synced_ref(sandbox)]).is_err() {
            let tip = self.tip(workspace)?;
            self.write_synced(sandbox, tip.as_str())?;
        }
        if ops::git(&self.root, &["rev-parse", "--verify", &sandbox_ref(sandbox)]).is_err() {
            // A clone that predates the saved ref stored the sandbox baseline
            // in the workspace's `@`; migrate that revision rather than the
            // shared tip, or the next snapshot would drop the sandbox's work.
            let saved = self.working_copy_tip(workspace)?;
            self.write_sandbox(sandbox, saved.as_str())?;
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

    fn resolve_begin(&self, workspace: &Workspace) -> Result<Begun, Error> {
        let ours = self.sandbox_tip(workspace)?;
        let theirs = self.tip(workspace)?;
        let path = self.workspace(sandbox_of(workspace));
        let _guard = self.lock()?;
        ops::jj_ok(
            &path,
            &[
                "new",
                ours.as_str(),
                theirs.as_str(),
                "-m",
                "litterbox: resolve the divergence",
            ],
        )?;
        Ok(Begun {
            revision: Revision::new(ops::commit_id(&path, "@")?),
        })
    }

    fn resolve_abandon(&self, workspace: &Workspace, ours: &Revision) -> Result<Revision, Error> {
        let _guard = self.lock()?;
        let sandbox = sandbox_of(workspace);
        let path = self.workspace(sandbox);
        // The resolution lived only in `@`; the saved baseline never moved.
        self.write_sandbox(sandbox, ours.as_str())?;
        ops::jj_ok(&path, &["new", ours.as_str()])?;
        Ok(Revision::new(ops::commit_id(&path, "@")?))
    }

    fn conflicts(&self, revision: &Revision) -> Result<Vec<Conflict>, Error> {
        Ok(ops::conflicts(&self.root, revision.as_str())?
            .into_iter()
            .map(|(path, sides)| {
                let hunks = ops::conflict_regions(&self.root, revision.as_str(), &path);
                Conflict {
                    path,
                    hunks,
                    sides,
                    base: true,
                }
            })
            .collect())
    }

    fn report(&self, workspace: &Workspace) -> Result<Report, Error> {
        let base = self.synced(workspace)?;
        let branch = self.tip(workspace)?;
        let sandbox = self.sandbox_tip(workspace)?;
        let path = clone::reconcile(&self.root, sandbox_of(workspace));
        let worktree = path.is_dir().then_some(path);
        let conflicts = merge_conflicts(&self.root, &base, &branch, &sandbox)?;
        Ok(Report {
            base,
            branch,
            sandbox: Some(sandbox),
            conflicts,
            worktree,
            reflog: None,
            next: None,
        })
    }

    fn stage(&self, workspace: &Workspace) -> Result<Staged, Error> {
        let _guard = self.lock()?;
        let sandbox = sandbox_of(workspace);
        let path = clone::reconcile(&self.root, sandbox);
        if path.exists() {
            return Err(Error::Backend {
                operation: "stage the merge".to_string(),
                message: format!(
                    "a staging workspace already exists at {}; finish or discard it first",
                    path.display()
                ),
            });
        }
        let base = self.synced(workspace)?;
        let branch = self.tip(workspace)?;
        let sandbox_tip = self.sandbox_tip(workspace)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(Error::Io)?;
        }
        let name = format!("{sandbox}-staging");
        let path_text = path.to_str().ok_or_else(|| Error::Backend {
            operation: "stage the merge".to_string(),
            message: "the staging path is not UTF-8".to_string(),
        })?;
        ops::jj_ok(
            &self.root,
            &[
                "workspace",
                "add",
                "--name",
                &name,
                "-r",
                base.as_str(),
                path_text,
            ],
        )?;
        ops::jj_ok(
            &path,
            &["new", branch.as_str(), "-m", "litterbox staging: theirs"],
        )?;
        let theirs = ops::commit_id(&path, "@")?;
        ops::jj_ok(
            &path,
            &["new", base.as_str(), "-m", "litterbox staging: ours"],
        )?;
        ops::jj_ok(&path, &["restore", "--from", sandbox_tip.as_str()])?;
        let ours = ops::commit_id(&path, "@")?;
        ops::jj_ok(
            &path,
            &["new", &theirs, &ours, "-m", "litterbox staging: merge"],
        )?;
        let conflicts = ops::conflicts(&path, "@")?
            .into_iter()
            .map(|(conflicted, _)| conflicted)
            .collect();
        Ok(Staged {
            path,
            branch,
            conflicts,
        })
    }

    fn finish(&self, workspace: &Workspace) -> Result<Finished, Error> {
        let _guard = self.lock()?;
        let sandbox = sandbox_of(workspace);
        let path = clone::reconcile(&self.root, sandbox);
        if !path.is_dir() {
            return Err(Error::Backend {
                operation: "finish the merge".to_string(),
                message: format!("no staging workspace at {}", path.display()),
            });
        }
        if !ops::conflicts(&path, "@")?.is_empty() {
            return Err(Error::Backend {
                operation: "finish the merge".to_string(),
                message: "the staging workspace has unresolved paths".to_string(),
            });
        }
        let merged = ops::commit_id(&path, "@")?;
        let tip = self.tip(workspace)?;
        if !self.is_ancestor(&tip, &Revision::new(&merged))? {
            return Err(Error::Backend {
                operation: "finish the merge".to_string(),
                message: format!(
                    "the staged revision {merged} does not descend from {}",
                    tip.as_str()
                ),
            });
        }
        let reference = format!("refs/heads/{}", workspace.branch().as_str());
        ops::git_update_ref(&self.root, &reference, &merged, tip.as_str())?;
        ops::jj_ok(&self.root, &["git", "import"])?;
        let name = format!("{sandbox}-staging");
        let _ = ops::jj_ok(&self.root, &["workspace", "forget", &name]);
        let _ = std::fs::remove_dir_all(&path);
        Ok(Finished::Landed {
            branch: Revision::new(merged),
        })
    }

    fn discard_stage(&self, workspace: &Workspace) -> Result<(), Error> {
        let _guard = self.lock()?;
        let sandbox = sandbox_of(workspace);
        let path = clone::reconcile(&self.root, sandbox);
        let name = format!("{sandbox}-staging");
        let _ = ops::jj_ok(&self.root, &["workspace", "forget", &name]);
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(&path);
        }
        Ok(())
    }

    fn staging(&self, workspace: &Workspace) -> Option<PathBuf> {
        let path = clone::reconcile(&self.root, sandbox_of(workspace));
        path.is_dir().then_some(path)
    }

    fn conflict_sides(&self, revision: &Revision, _path: &Path) -> Result<ConflictSides, Error> {
        let (base, ours, theirs) = ops::conflict_sides(&self.root, revision.as_str())?;
        Ok(ConflictSides {
            base: base.map(TreeId::new),
            ours: TreeId::new(ours),
            theirs: TreeId::new(theirs),
        })
    }

    fn resolve(&self, _revision: &Revision, _paths: &[PathBuf]) -> Result<Revision, Error> {
        Err(unsupported("resolve"))
    }

    fn abort(&self, _revision: &Revision) -> Result<(), Error> {
        Err(unsupported("abort"))
    }

    fn history(&self, workspace: &Workspace) -> Result<Vec<HistoryStep>, Error> {
        let _guard = self.lock()?;
        history::read(&self.root, workspace.branch().as_str())
    }

    fn restore(&self, workspace: &Workspace, revision: &Revision) -> Result<Restored, Error> {
        let _guard = self.lock()?;
        restore::restore(self, workspace, sandbox_of(workspace), revision)
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
            config: None,
        })
    }

    /// Attach a config string for tests that do not go through `open`.
    pub(crate) fn with_config(mut self, config: Option<String>) -> Self {
        self.config = config;
        self
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use super::JjVcs;
    use crate::vcs::{Revision, Vcs};

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
    fn restore_adds_one_forward_commit() {
        unsafe {
            std::env::set_var("JJ_USER", "Test");
            std::env::set_var("JJ_EMAIL", "test@example.com");
        }

        let host = repository();
        let data = TempDir::new().expect("data");
        let root = data.path().join("clone");
        let vcs = JjVcs::open_at(root.clone(), host.path().to_path_buf()).expect("backend");
        let workspace = vcs.prepare("demo").expect("prepare").workspace;
        let base = vcs.tip(&workspace).expect("base").as_str().to_string();

        jj(&root, &["new", &base, "-m", "one"]);
        std::fs::write(root.join("file.txt"), "one\n").expect("write");
        let one = super::ops::commit_id(&root, "@").expect("one");
        vcs.set_tip(&workspace, &Revision::new(one.clone()))
            .expect("tip");

        let restored = vcs
            .restore(&workspace, &Revision::new(one.clone()))
            .expect("restore");
        assert!(restored.created);
        assert_eq!(vcs.tip(&workspace).expect("tip"), restored.revision);
        assert_eq!(
            vcs.sandbox_tip(&workspace).expect("sandbox tip"),
            restored.revision
        );
        assert_eq!(vcs.synced(&workspace).expect("synced"), restored.revision);
        let written =
            std::fs::read_to_string(vcs.workspace("demo").join("file.txt")).expect("read");
        assert_eq!(written, "one\n");
        assert!(
            vcs.conflicts(&restored.revision)
                .expect("conflicts")
                .is_empty(),
            "the restore commit is clean"
        );

        let again = vcs
            .restore(&workspace, &Revision::new(one))
            .expect("second restore");
        assert!(!again.created, "the tree already matches");
        assert_eq!(again.revision, restored.revision);
    }

    #[test]
    fn history_is_scoped_to_the_sandbox_bookmark() {
        unsafe {
            std::env::set_var("JJ_USER", "Test");
            std::env::set_var("JJ_EMAIL", "test@example.com");
        }

        let host = repository();
        let data = TempDir::new().expect("data");
        let root = data.path().join("clone");
        let vcs = JjVcs::open_at(root.clone(), host.path().to_path_buf()).expect("backend");
        let demo = vcs.prepare("demo").expect("demo").workspace;
        let other = vcs.prepare("other").expect("other").workspace;
        let base = vcs.tip(&demo).expect("base").as_str().to_string();

        jj(&root, &["new", &base, "-m", "one"]);
        let one = super::ops::commit_id(&root, "@").expect("one");
        jj(&root, &["new", &one, "-m", "two"]);
        let two = super::ops::commit_id(&root, "@").expect("two");
        vcs.set_tip(&demo, &Revision::new(one.clone()))
            .expect("one");
        vcs.set_tip(&demo, &Revision::new(two.clone()))
            .expect("two");

        jj(&root, &["new", &base, "-m", "other"]);
        let other_commit = super::ops::commit_id(&root, "@").expect("other");
        vcs.set_tip(&other, &Revision::new(other_commit.clone()))
            .expect("other");

        let before = super::ops::jj(
            &root,
            &[
                "--ignore-working-copy",
                "op",
                "log",
                "--limit",
                "1",
                "-T",
                "self.id()",
            ],
        )
        .expect("op log");
        let steps = vcs.history(&demo).expect("history");
        let after = super::ops::jj(
            &root,
            &[
                "--ignore-working-copy",
                "op",
                "log",
                "--limit",
                "1",
                "-T",
                "self.id()",
            ],
        )
        .expect("op log");

        assert_eq!(before, after, "history writes no operation");
        assert_eq!(steps[0].revision.as_str(), two, "{steps:?}");
        assert!(
            steps.iter().any(|step| step.revision.as_str() == one),
            "{steps:?}"
        );
        assert!(
            steps
                .iter()
                .all(|step| step.revision.as_str() != other_commit),
            "another bookmark's commit is not a demo step: {steps:?}"
        );
    }

    #[test]
    fn the_config_string_is_applied_to_the_clone() {
        unsafe {
            std::env::set_var("JJ_USER", "Test");
            std::env::set_var("JJ_EMAIL", "test@example.com");
        }

        let host = repository();
        let data = TempDir::new().expect("data");
        let root = data.path().join("clone");
        let vcs = JjVcs::open_at(root.clone(), host.path().to_path_buf())
            .expect("backend")
            .with_config(Some("[ui]\ncolor = \"never\"\n".to_string()));
        vcs.prepare("demo").expect("prepare");

        let value = super::ops::jj(&root, &["config", "get", "ui.color"]).expect("config");
        assert!(value.contains("never"), "{value}");
    }

    #[test]
    fn an_invalid_config_is_refused_when_the_clone_is_prepared() {
        unsafe {
            std::env::set_var("JJ_USER", "Test");
            std::env::set_var("JJ_EMAIL", "test@example.com");
        }

        let host = repository();
        let data = TempDir::new().expect("data");
        let vcs = JjVcs::open_at(data.path().join("clone"), host.path().to_path_buf())
            .expect("backend")
            .with_config(Some("not toml".to_string()));
        let error = vcs.prepare("demo").expect_err("invalid config");
        assert!(error.to_string().contains("config"), "{error}");
    }

    fn numbered(label: &str) -> String {
        let mut text = String::new();
        for index in 1..=24 {
            if index == 1 || index == 20 {
                text.push_str(&format!("{label} {index}\n"));
            } else {
                text.push_str(&format!("filler {index}\n"));
            }
        }
        text
    }

    fn jj(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("jj")
            .current_dir(dir)
            .args(args)
            .status()
            .expect("jj runs");
        assert!(status.success(), "jj {args:?}");
    }

    #[test]
    fn resolve_begin_writes_a_conflicted_merge() {
        unsafe {
            std::env::set_var("JJ_USER", "Test");
            std::env::set_var("JJ_EMAIL", "test@example.com");
        }

        let host = repository();
        let data = TempDir::new().expect("data");
        let root = data.path().join("clone");
        let vcs = JjVcs::open_at(root.clone(), host.path().to_path_buf()).expect("backend");
        let prepared = vcs.prepare("demo").expect("prepare");
        let workspace = prepared.workspace.clone();

        std::fs::write(root.join("file.txt"), numbered("base")).expect("write");
        jj(&root, &["describe", "-m", "base"]);
        let base = super::ops::commit_id(&root, "@").expect("base id");

        jj(&root, &["new", &base, "-m", "ours"]);
        std::fs::write(root.join("file.txt"), numbered("ours")).expect("write");
        let ours = super::ops::commit_id(&root, "@").expect("ours id");

        jj(&root, &["new", &base, "-m", "theirs"]);
        std::fs::write(root.join("file.txt"), numbered("theirs")).expect("write");
        let theirs = super::ops::commit_id(&root, "@").expect("theirs id");

        vcs.set_synced(&workspace, &Revision::new(base))
            .expect("synced");
        vcs.set_sandbox_tip(&workspace, &Revision::new(ours.clone()))
            .expect("sandbox tip");
        vcs.set_tip(&workspace, &Revision::new(theirs))
            .expect("branch tip");

        let begun = vcs.resolve_begin(&workspace).expect("begin");
        let revision = begun.revision;
        let conflicts = vcs.conflicts(&revision).expect("conflicts");
        assert_eq!(conflicts.len(), 1, "{conflicts:?}");
        assert_eq!(conflicts[0].path, Path::new("file.txt"));
        assert_eq!(conflicts[0].sides, 2);
        assert_eq!(
            vcs.working_copy_tip(&workspace).expect("working copy tip"),
            revision,
            "the working copy holds the conflict"
        );
        assert_eq!(
            vcs.sandbox_tip(&workspace).expect("sandbox tip"),
            Revision::new(ours.clone()),
            "the saved baseline is not rewritten by the resolution"
        );

        let restored = vcs
            .resolve_abandon(&workspace, &Revision::new(ours.clone()))
            .expect("abandon");
        assert!(
            vcs.conflicts(&restored).expect("conflicts").is_empty(),
            "the restored revision carries no conflict"
        );
        assert_eq!(
            vcs.working_copy_tip(&workspace).expect("working copy tip"),
            restored,
            "the working copy is back on the sandbox side"
        );
        assert_eq!(
            vcs.sandbox_tip(&workspace).expect("sandbox tip"),
            Revision::new(ours),
            "abandon restores the saved baseline"
        );

        vcs.set_tip(&workspace, &revision).expect("conflicted tip");
        let error = vcs
            .restore(&workspace, &restored)
            .expect_err("a conflicted shared commit is refused");
        assert!(error.to_string().contains("conflicted"), "{error}");
    }

    #[test]
    fn a_blob_reads_one_path_from_one_tree() {
        unsafe {
            std::env::set_var("JJ_USER", "Test");
            std::env::set_var("JJ_EMAIL", "test@example.com");
        }

        let host = repository();
        let data = TempDir::new().expect("data");
        let vcs =
            JjVcs::open_at(data.path().join("clone"), host.path().to_path_buf()).expect("backend");
        let prepared = vcs.prepare("demo").expect("prepare");
        let tree = vcs.tree(&prepared.base).expect("tree");

        let content = vcs
            .blob(&tree, Path::new("README.md"))
            .expect("blob")
            .expect("the file is in the tree");
        assert_eq!(content, b"hello");

        let absent = vcs.blob(&tree, Path::new("absent.txt")).expect("blob");
        assert!(absent.is_none(), "a path the tree does not hold is None");
    }

    #[test]
    fn conflicts_report_their_sides_and_regions() {
        unsafe {
            std::env::set_var("JJ_USER", "Test");
            std::env::set_var("JJ_EMAIL", "test@example.com");
        }

        let host = repository();
        let data = TempDir::new().expect("data");
        let root = data.path().join("clone");
        let vcs = JjVcs::open_at(root.clone(), host.path().to_path_buf()).expect("backend");
        vcs.prepare("demo").expect("prepare");

        std::fs::write(root.join("file.txt"), numbered("base")).expect("write");
        jj(&root, &["describe", "-m", "base"]);
        let base = super::ops::commit_id(&root, "@").expect("base id");

        jj(&root, &["new", &base, "-m", "theirs"]);
        std::fs::write(root.join("file.txt"), numbered("theirs")).expect("write");
        let theirs = super::ops::commit_id(&root, "@").expect("theirs id");

        jj(&root, &["new", &base, "-m", "ours"]);
        std::fs::write(root.join("file.txt"), numbered("ours")).expect("write");
        let ours = super::ops::commit_id(&root, "@").expect("ours id");

        jj(&root, &["new", &ours, &theirs, "-m", "merge"]);
        let merge = super::ops::commit_id(&root, "@").expect("merge id");

        let conflicts = vcs
            .conflicts(&Revision::new(merge.clone()))
            .expect("conflicts");
        assert_eq!(conflicts.len(), 1, "{conflicts:?}");
        assert_eq!(conflicts[0].path, Path::new("file.txt"));
        assert_eq!(conflicts[0].sides, 2, "both sides changed the file");
        assert_eq!(conflicts[0].hunks, 2, "each side rewrote two regions");
        assert!(conflicts[0].base, "the merge has a common ancestor");

        let sides = vcs
            .conflict_sides(&Revision::new(merge), Path::new("file.txt"))
            .expect("sides");
        let side = |tree: &crate::vcs::TreeId| {
            String::from_utf8(
                vcs.blob(tree, Path::new("file.txt"))
                    .expect("blob")
                    .expect("present"),
            )
            .expect("utf8")
        };
        assert!(
            side(&sides.ours).starts_with("ours 1"),
            "{}",
            side(&sides.ours)
        );
        assert!(side(&sides.theirs).starts_with("theirs 1"));
        assert!(side(sides.base.as_ref().expect("base")).starts_with("base 1"));

        let clean = vcs.conflicts(&Revision::new(ours)).expect("clean");
        assert!(clean.is_empty(), "an unconflicted revision reports nothing");
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
        let vcs =
            JjVcs::open_at(data.path().join("clone"), host.path().to_path_buf()).expect("backend");

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
        let snapshot = vcs
            .snapshot(&workspace, staging.path(), "snap")
            .expect("snapshot");
        assert_ne!(snapshot, prepared.base);
        assert_eq!(vcs.sandbox_tip(&workspace).expect("sandbox tip"), snapshot);

        vcs.advance(&workspace, &snapshot, &prepared.base)
            .expect("advance");
        assert_eq!(vcs.tip(&workspace).expect("tip"), snapshot);
        vcs.set_synced(&workspace, &snapshot)
            .expect("record synced");
        assert_eq!(vcs.synced(&workspace).expect("synced"), snapshot);
    }

    #[test]
    fn a_snapshot_never_rewrites_the_published_revision() {
        unsafe {
            std::env::set_var("JJ_USER", "Test");
            std::env::set_var("JJ_EMAIL", "test@example.com");
        }

        let host = repository();
        let data = TempDir::new().expect("data");
        let vcs =
            JjVcs::open_at(data.path().join("clone"), host.path().to_path_buf()).expect("backend");
        let prepared = vcs.prepare("demo").expect("prepare");
        let workspace = prepared.workspace.clone();

        let staging = TempDir::new().expect("staging");
        std::fs::write(staging.path().join("README.md"), "one").expect("write");
        let first = vcs
            .snapshot(&workspace, staging.path(), "one")
            .expect("snapshot");
        vcs.advance(&workspace, &first, &prepared.base)
            .expect("advance");
        vcs.set_synced(&workspace, &first).expect("synced");

        // An edit in the workspace is not the saved baseline, and reading the
        // baseline must not snapshot that edit into it.
        std::fs::write(vcs.workspace("demo").join("README.md"), "two").expect("write");
        assert_eq!(
            vcs.sandbox_tip(&workspace).expect("saved"),
            first,
            "reading the baseline does not snapshot the working copy"
        );
        let working = vcs.working_copy_tip(&workspace).expect("working");
        assert_ne!(working, first, "the edit is in the working copy");
        assert_ne!(
            vcs.tree(&working).expect("working tree"),
            vcs.tree(&first).expect("saved tree")
        );

        // The next snapshot publishes a descendant rather than rewriting the
        // revision the shared branch already points at.
        std::fs::write(staging.path().join("README.md"), "two").expect("write");
        let second = vcs
            .snapshot(&workspace, staging.path(), "two")
            .expect("second snapshot");
        assert_ne!(second, first);
        assert!(
            vcs.is_ancestor(&first, &second).expect("ancestry"),
            "the published revision survives the later snapshot"
        );
        assert_eq!(vcs.sandbox_tip(&workspace).expect("saved"), second);
    }

    #[test]
    fn staging_matches_merge_tree_and_discards() {
        unsafe {
            std::env::set_var("JJ_USER", "Test");
            std::env::set_var("JJ_EMAIL", "test@example.com");
        }

        let host = repository();
        let data = TempDir::new().expect("data");
        let vcs =
            JjVcs::open_at(data.path().join("clone"), host.path().to_path_buf()).expect("backend");
        let prepared = vcs.prepare("demo").expect("prepare");
        let workspace = prepared.workspace.clone();

        let staging = TempDir::new().expect("staging");
        std::fs::write(staging.path().join("README.md"), "hello").expect("write");
        std::fs::write(staging.path().join("file.txt"), "sandbox\n").expect("write");
        let sandbox = vcs
            .snapshot(&workspace, staging.path(), "sandbox")
            .expect("snapshot");

        super::ops::jj_ok(&vcs.root, &["new", prepared.base.as_str(), "-m", "user"]).expect("new");
        std::fs::write(vcs.root.join("file.txt"), "user\n").expect("write");
        super::ops::jj_ok(&vcs.root, &["bookmark", "set", "litterbox/demo", "-r", "@"])
            .expect("bookmark");
        let branch = vcs.tip(&workspace).expect("tip");
        assert_ne!(branch, prepared.base);
        assert_ne!(branch, sandbox);

        let report = vcs.report(&workspace).expect("report");
        assert_eq!(report.conflicts, vec![std::path::PathBuf::from("file.txt")]);
        assert!(report.worktree.is_none(), "nothing is staged yet");

        let staged = vcs.stage(&workspace).expect("stage");
        assert_eq!(staged.conflicts, vec![std::path::PathBuf::from("file.txt")]);
        assert!(staged.path.is_dir());

        let second = vcs.stage(&workspace).expect_err("a second stage refuses");
        assert!(second.to_string().contains("already exists"), "{second}");

        vcs.discard_stage(&workspace).expect("discard");
        assert!(!super::clone::reconcile(&vcs.root, "demo").exists());
    }

    #[test]
    fn a_sandbox_is_listed_exposed_deleted_and_pruned() {
        unsafe {
            std::env::set_var("JJ_USER", "Test");
            std::env::set_var("JJ_EMAIL", "test@example.com");
        }

        let host = repository();
        let data = TempDir::new().expect("data");
        let vcs =
            JjVcs::open_at(data.path().join("clone"), host.path().to_path_buf()).expect("backend");
        vcs.prepare("demo").expect("prepare");

        assert_eq!(vcs.sandboxes().expect("list"), vec!["demo".to_string()]);
        assert!(vcs.expose("litterbox").expect("expose"));
        assert!(!vcs.expose("litterbox").expect("already present"));

        vcs.delete_branch("demo").expect("delete");
        assert!(vcs.sandboxes().expect("list").is_empty());
        assert!(
            vcs.delete_branch("demo").is_err(),
            "a gone sandbox is NotFound"
        );

        vcs.prune().expect("prune");
        assert!(!vcs.root.join(".jj").exists(), "the clone is released");
    }
}
