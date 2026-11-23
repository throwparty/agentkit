use std::sync::Arc;

use tempfile::TempDir;

use crate::metadata_store::{self, LoadedSandbox, StoredSandbox};
use crate::orchestrator::resolve::Resolver;
use crate::sandbox::contract::{Backend, SandboxRecord, WorkspaceBinding};
use crate::sandbox::selected::Selected;
use crate::testing::TestBackend;
use crate::testing::vcs::TestVcs;
use crate::vcs::Vcs;

fn selected() -> Selected {
    let core: Arc<dyn Backend> = Arc::new(TestBackend::new());
    Selected::new("test", core)
}

fn current(loaded: LoadedSandbox) -> StoredSandbox {
    match loaded {
        LoadedSandbox::Current(stored) => *stored,
        LoadedSandbox::Corrupt { path, reason } => {
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
        binding: Some(WorkspaceBinding::Bound),
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
        .with_conflicts_for(branch, conflicts)
        .with_conflicts_for("resolution-revision", conflicts);
    let vcs = if left.is_empty() {
        vcs
    } else {
        vcs.with_conflicts_for(sandbox, left)
    };
    Arc::new(vcs)
}

#[tokio::test]
async fn begin_records_the_resolution_and_a_second_begin_is_refused() {
    let dir = TempDir::new().expect("tempdir");
    let stored = stored();
    let selected = selected();
    let vcs = vcs_with("branch-tip", &["file.txt"], "sandbox-tip", &[]);

    let began = Resolver::new(&vcs, &stored, &selected, dir.path())
        .begin()
        .await
        .expect("begin");
    assert_eq!(began.synced.as_str(), "synced-revision");
    assert_eq!(began.conflicts.len(), 1);

    let reloaded = current(
        metadata_store::load_sandbox_at(dir.path(), "demo")
            .expect("load")
            .expect("present"),
    );
    assert_eq!(
        reloaded.resolution.clone().expect("recorded").synced,
        "synced-revision",
        "the record outlives the call"
    );

    let refused = Resolver::new(&vcs, &reloaded, &selected, dir.path())
        .begin()
        .await;
    assert!(refused.is_err(), "a second begin must be refused");
}

#[tokio::test]
async fn begin_without_a_conflict_reports_the_state() {
    let dir = TempDir::new().expect("tempdir");
    let stored = stored();
    let selected = selected();
    let vcs = vcs_with("branch-tip", &[], "sandbox-tip", &[]);

    let error = Resolver::new(&vcs, &stored, &selected, dir.path())
        .begin()
        .await
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

#[tokio::test]
async fn finish_refuses_while_conflicts_remain_and_clears_when_they_are_gone() {
    let dir = TempDir::new().expect("tempdir");
    let selected = selected();
    let dirty = vcs_with("branch-tip", &["file.txt"], "sandbox-tip", &["file.txt"]);
    let record = stored();
    let resolver = Resolver::new(&dirty, &record, &selected, dir.path());
    resolver.begin().await.expect("begin");
    assert!(
        resolver.finish().is_err(),
        "finish refuses while the working copy conflicts"
    );

    let clean = TempDir::new().expect("tempdir");
    let resolved = vcs_with("branch-tip", &["file.txt"], "sandbox-tip", &[]);
    let record = stored();
    let resolver = Resolver::new(&resolved, &record, &selected, clean.path());
    resolver.begin().await.expect("begin");
    resolver.finish().expect("finish");
    let reloaded = current(
        metadata_store::load_sandbox_at(clean.path(), "demo")
            .expect("load")
            .expect("present"),
    );
    assert!(reloaded.resolution.is_none(), "finish clears it");
}

#[tokio::test]
async fn abandon_clears_the_record() {
    let dir = TempDir::new().expect("tempdir");
    let selected = selected();
    let vcs = vcs_with("branch-tip", &["file.txt"], "sandbox-tip", &["file.txt"]);
    let record = stored();
    let resolver = Resolver::new(&vcs, &record, &selected, dir.path());
    resolver.begin().await.expect("begin");
    resolver.abandon().expect("abandon");

    let reloaded = current(
        metadata_store::load_sandbox_at(dir.path(), "demo")
            .expect("load")
            .expect("present"),
    );
    assert!(reloaded.resolution.is_none());
    assert!(
        Resolver::new(&vcs, &reloaded, &selected, dir.path())
            .abandon()
            .is_err(),
        "abandon with nothing open is refused"
    );
}
