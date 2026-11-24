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
        .with_conflicts_for(branch, conflicts);
    let vcs = if left.is_empty() {
        vcs
    } else {
        vcs.with_conflicts_for("resolution-revision", left)
    };
    Arc::new(vcs)
}

#[tokio::test]
async fn begin_records_the_resolution_and_a_second_begin_is_refused() {
    let dir = TempDir::new().expect("tempdir");
    let stored = stored();
    let selected = selected();
    let vcs = vcs_with("branch-tip", &[], "sandbox-tip", &["file.txt"]);

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

    let second = Resolver::new(&vcs, &reloaded, &selected, dir.path())
        .begin()
        .await;
    assert!(second.is_err(), "a second begin must fail");
}

#[tokio::test]
async fn begin_fails_on_a_shared_branch_tip_that_already_conflicts() {
    let dir = TempDir::new().expect("tempdir");
    let selected = selected();
    let mut stored = stored();
    stored.vcs = Some("jj".to_string());
    let vcs = vcs_with("branch-tip", &["file.txt"], "sandbox-tip", &[]);

    let error = Resolver::new(&vcs, &stored, &selected, dir.path())
        .begin()
        .await
        .expect_err("a broken tip is the owner's to fix");

    let message = error.to_string();
    assert!(
        message.contains("resolves only the merge it creates"),
        "{message}"
    );
    assert!(message.contains("jj edit"), "it names the fix: {message}");
    assert!(
        metadata_store::load_sandbox_at(dir.path(), "demo")
            .expect("load")
            .is_none(),
        "a failed begin writes no record"
    );
}

#[tokio::test]
async fn the_error_names_a_git_fix_for_a_git_sandbox() {
    let dir = TempDir::new().expect("tempdir");
    let selected = selected();
    let mut stored = stored();
    stored.vcs = Some("git".to_string());
    let vcs = vcs_with("branch-tip", &["file.txt"], "sandbox-tip", &[]);

    let error = Resolver::new(&vcs, &stored, &selected, dir.path())
        .begin()
        .await
        .expect_err("a broken tip is the owner's to fix");

    let message = error.to_string();
    assert!(
        message.contains("git commit --amend"),
        "the fix matches the tool: {message}"
    );
    assert!(
        !message.contains("jj edit"),
        "a Git sandbox is not told to run jj: {message}"
    );
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
        error.to_string().contains("without a conflict"),
        "the message names the state: {error}"
    );
    assert!(
        metadata_store::load_sandbox_at(dir.path(), "demo")
            .expect("load")
            .is_none(),
        "a failed begin writes no record"
    );
}

#[tokio::test]
async fn finish_fails_while_conflicts_remain_and_clears_when_they_are_gone() {
    let dir = TempDir::new().expect("tempdir");
    let selected = selected();
    let dirty = vcs_with("branch-tip", &[], "sandbox-tip", &["file.txt"]);
    let record = stored();
    let resolver = Resolver::new(&dirty, &record, &selected, dir.path());
    resolver.begin().await.expect("begin");
    assert!(
        resolver.finish().is_err(),
        "finish fails while the working copy conflicts"
    );

    let clean = TempDir::new().expect("tempdir");
    let resolved = Arc::new(
        TestVcs::new()
            .with_tip("branch-tip")
            .with_sandbox_tip("sandbox-tip")
            .with_synced("synced-revision")
            .with_conflicts_for("resolution-revision", &["file.txt"]),
    );
    let as_dyn: Arc<dyn Vcs> = resolved.clone();
    let record = stored();
    let resolver = Resolver::new(&as_dyn, &record, &selected, clean.path());
    resolver.begin().await.expect("begin");
    resolved.clear_conflicts("resolution-revision");
    resolver.finish().expect("finish");
    let reloaded = current(
        metadata_store::load_sandbox_at(clean.path(), "demo")
            .expect("load")
            .expect("present"),
    );
    assert!(reloaded.resolution.is_none(), "finish clears it");
}

#[tokio::test]
async fn finish_reports_a_path_the_resolution_dropped() {
    let dir = TempDir::new().expect("tempdir");
    let selected = selected();
    let test = Arc::new(
        TestVcs::new()
            .with_tip("branch-tip")
            .with_sandbox_tip("sandbox-tip")
            .with_synced("synced-revision")
            .with_conflicts_for("resolution-revision", &["file.txt"])
            .with_tree("resolution-revision", "resolved-tree")
            .with_tree("branch-tip", "theirs-tree")
            .with_tree("sandbox-tip", "ours-tree")
            .with_deleted("ours-tree", "resolved-tree", "ours-only.txt"),
    );
    let vcs: Arc<dyn Vcs> = test.clone();

    let record = stored();
    let resolver = Resolver::new(&vcs, &record, &selected, dir.path());
    resolver.begin().await.expect("begin");
    test.clear_conflicts("resolution-revision");
    let finished = resolver.finish().expect("finish");

    assert_eq!(
        finished.dropped_from_ours,
        vec![std::path::PathBuf::from("ours-only.txt")],
        "the report names what the resolution lost"
    );
    assert!(
        finished.dropped_from_theirs.is_empty(),
        "the other side lost nothing"
    );
}

#[tokio::test]
async fn abandon_clears_the_record() {
    let dir = TempDir::new().expect("tempdir");
    let selected = selected();
    let vcs = vcs_with("branch-tip", &[], "sandbox-tip", &["file.txt"]);
    let record = stored();
    let resolver = Resolver::new(&vcs, &record, &selected, dir.path());
    resolver.begin().await.expect("begin");
    resolver.abandon().await.expect("abandon");

    let reloaded = current(
        metadata_store::load_sandbox_at(dir.path(), "demo")
            .expect("load")
            .expect("present"),
    );
    assert!(reloaded.resolution.is_none());
    assert!(
        Resolver::new(&vcs, &reloaded, &selected, dir.path())
            .abandon()
            .await
            .is_err(),
        "abandon with nothing open fails"
    );
}
