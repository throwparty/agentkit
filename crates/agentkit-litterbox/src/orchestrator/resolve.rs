//! Resolving a conflict inside the sandbox.
//!
//! Begin opens a resolution and writes the conflicted revision into the
//! workspace, finish asks the backend whether the conflict is gone and performs
//! the host write, and abandon writes the merge again. The revision under
//! resolution is read from the working copy on every operation, never stored,
//! because resolving rewrites it.

use std::path::Path;
use std::sync::Arc;

use crate::metadata_store::{self, StoredResolution, StoredSandbox};
use crate::orchestrator::service::contract_error;
use crate::sandbox::contract::Error;
use crate::vcs::{Conflict, Revision, Vcs, Workspace};

/// What one resolution action achieved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Resolved {
    /// The revision both sides agreed on when resolution began.
    pub(crate) synced: Revision,
    /// The conflicts the working copy still carries.
    pub(crate) conflicts: Vec<Conflict>,
}

/// The actions one sandbox's resolution can take.
pub(crate) struct Resolver<'a> {
    vcs: &'a Arc<dyn Vcs>,
    stored: &'a StoredSandbox,
    records_dir: &'a Path,
    resolution: std::sync::Mutex<Option<StoredResolution>>,
}

impl<'a> Resolver<'a> {
    /// Build a resolver for one sandbox.
    pub(crate) fn new(
        vcs: &'a Arc<dyn Vcs>,
        stored: &'a StoredSandbox,
        records_dir: &'a Path,
    ) -> Self {
        Self {
            vcs,
            stored,
            records_dir,
            resolution: std::sync::Mutex::new(stored.resolution.clone()),
        }
    }

    fn workspace(&self) -> Workspace {
        Workspace::from_branch(&self.stored.branch_name)
    }

    fn open(&self) -> Result<StoredResolution, Error> {
        self.resolution
            .lock()
            .map_err(|_| Error::failed("the resolution lock is poisoned".to_string()))?
            .clone()
            .ok_or_else(|| {
                Error::failed(
                    "no resolution is open for this sandbox; call begin first".to_string(),
                )
            })
    }

    fn record(&self, resolution: Option<StoredResolution>) -> Result<(), Error> {
        let mut updated = self.stored.clone();
        updated.resolution = resolution.clone();
        metadata_store::store_sandbox_at(self.records_dir, &updated.sandbox.name, &updated)?;
        *self
            .resolution
            .lock()
            .map_err(|_| Error::failed("the resolution lock is poisoned".to_string()))? =
            resolution;
        Ok(())
    }

    /// Open a resolution and report the conflicts it has to settle.
    pub(crate) fn begin(&self) -> Result<Resolved, Error> {
        if let Some(open) = self.resolution.lock().ok().and_then(|open| open.clone()) {
            return Err(Error::failed(format!(
                "a resolution is already open from {}",
                open.synced
            )));
        }
        let workspace = self.workspace();
        let branch = self.vcs.tip(&workspace).map_err(contract_error)?;
        let conflicts = self.vcs.conflicts(&branch).map_err(contract_error)?;
        if conflicts.is_empty() {
            return Err(Error::failed(
                "the shared branch carries no conflict to resolve".to_string(),
            ));
        }
        let synced = self.vcs.synced(&workspace).map_err(contract_error)?;
        self.record(Some(StoredResolution {
            synced: synced.as_str().to_string(),
        }))?;
        Ok(Resolved { synced, conflicts })
    }

    /// Close a resolution, refusing while the working copy still conflicts.
    pub(crate) fn finish(&self) -> Result<Resolved, Error> {
        let open = self.open()?;
        let workspace = self.workspace();
        let revision = self.vcs.sandbox_tip(&workspace).map_err(contract_error)?;
        let conflicts = self.vcs.conflicts(&revision).map_err(contract_error)?;
        if !conflicts.is_empty() {
            return Err(Error::failed(format!(
                "{} conflicted paths remain; resolve them before finishing",
                conflicts.len()
            )));
        }
        self.record(None)?;
        Ok(Resolved {
            synced: Revision::new(open.synced),
            conflicts,
        })
    }

    /// Close a resolution and leave the sandbox on its own branch.
    pub(crate) fn abandon(&self) -> Result<Resolved, Error> {
        let open = self.open()?;
        self.record(None)?;
        Ok(Resolved {
            synced: Revision::new(open.synced),
            conflicts: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tempfile::TempDir;

    use super::Resolver;
    use crate::metadata_store::{self, StoredSandbox};
    use crate::sandbox::contract::SandboxRecord;
    use crate::testing::vcs::TestVcs;
    use crate::vcs::Vcs;

    fn current(loaded: &crate::metadata_store::LoadedSandbox) -> &StoredSandbox {
        match loaded {
            crate::metadata_store::LoadedSandbox::Current(stored) => stored,
            crate::metadata_store::LoadedSandbox::Corrupt { path, reason } => {
                panic!("the record at {} is corrupt: {reason}", path.display())
            }
        }
    }

    fn stored() -> StoredSandbox {
        StoredSandbox {
            sandbox: SandboxRecord {
                name: "demo".to_string(),
                backend: "test".to_string(),
                project_slug: "project".to_string(),
                data: toml::Value::Table(toml::Table::new()),
            },
            branch_name: "litterbox/demo".to_string(),
            remote: false,
            binding: None,
            vcs: None,
            merge: None,
            resolution: None,
        }
    }

    fn vcs_with(branch: &str, conflicts: &[&str], sandbox: &str, left: &[&str]) -> Arc<dyn Vcs> {
        let vcs = TestVcs::new()
            .with_tip(branch)
            .with_sandbox_tip(sandbox)
            .with_synced("synced-revision")
            .with_conflicts_for(branch, conflicts);
        let vcs = if left.is_empty() {
            vcs
        } else {
            vcs.with_conflicts_for(sandbox, left)
        };
        Arc::new(vcs)
    }

    #[test]
    fn begin_records_the_resolution_and_a_second_begin_is_refused() {
        let dir = TempDir::new().expect("tempdir");
        let stored = stored();
        let vcs = vcs_with("branch-tip", &["file.txt"], "sandbox-tip", &[]);

        let resolver = Resolver::new(&vcs, &stored, dir.path());
        let began = resolver.begin().expect("begin");
        assert_eq!(began.synced.as_str(), "synced-revision");
        assert_eq!(began.conflicts.len(), 1);

        let reloaded = metadata_store::load_sandbox_at(dir.path(), "demo")
            .expect("load")
            .expect("present");
        assert_eq!(
            current(&reloaded)
                .resolution
                .clone()
                .expect("recorded")
                .synced,
            "synced-revision",
            "the record outlives the call"
        );

        let refused = Resolver::new(&vcs, current(&reloaded), dir.path()).begin();
        assert!(refused.is_err(), "a second begin must be refused");
    }

    #[test]
    fn begin_without_a_conflict_reports_the_state() {
        let dir = TempDir::new().expect("tempdir");
        let stored = stored();
        let vcs = vcs_with("branch-tip", &[], "sandbox-tip", &[]);

        let error = Resolver::new(&vcs, &stored, dir.path())
            .begin()
            .expect_err("no conflict");
        assert!(
            error.to_string().contains("no conflict"),
            "the message names the state: {error}"
        );
        assert!(
            metadata_store::load_sandbox_at(dir.path(), "demo")
                .expect("load")
                .is_none(),
            "a refused begin writes no record"
        );
    }

    #[test]
    fn finish_refuses_while_conflicts_remain_and_clears_when_they_are_gone() {
        let dir = TempDir::new().expect("tempdir");
        let dirty = vcs_with("branch-tip", &["file.txt"], "sandbox-tip", &["file.txt"]);
        let record = stored();
        let resolver = Resolver::new(&dirty, &record, dir.path());
        resolver.begin().expect("begin");
        assert!(
            resolver.finish().is_err(),
            "finish refuses while the working copy conflicts"
        );

        let clean = TempDir::new().expect("tempdir");
        let resolved = vcs_with("branch-tip", &["file.txt"], "sandbox-tip", &[]);
        let record = stored();
        let resolver = Resolver::new(&resolved, &record, clean.path());
        resolver.begin().expect("begin");
        resolver.finish().expect("finish");
        let reloaded = metadata_store::load_sandbox_at(clean.path(), "demo")
            .expect("load")
            .expect("present");
        assert!(current(&reloaded).resolution.is_none(), "finish clears it");
    }

    #[test]
    fn abandon_clears_the_record() {
        let dir = TempDir::new().expect("tempdir");
        let vcs = vcs_with("branch-tip", &["file.txt"], "sandbox-tip", &["file.txt"]);
        let record = stored();
        let resolver = Resolver::new(&vcs, &record, dir.path());
        resolver.begin().expect("begin");
        resolver.abandon().expect("abandon");

        let reloaded = metadata_store::load_sandbox_at(dir.path(), "demo")
            .expect("load")
            .expect("present");
        assert!(current(&reloaded).resolution.is_none());
        assert!(
            Resolver::new(&vcs, current(&reloaded), dir.path())
                .abandon()
                .is_err(),
            "abandon with nothing open is refused"
        );
    }
}
