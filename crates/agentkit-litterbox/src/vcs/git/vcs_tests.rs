//! Behavioural tests for the Git backend behind the contract.

use std::fs;

use git2::{IndexAddOption, Repository, Signature};
use tempfile::TempDir;

use crate::vcs::{ConflictModel, Error, Revision, Vcs};

use super::GitVcs;

fn repo_with_commit() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let repo = Repository::init(dir.path()).expect("init");
    fs::write(dir.path().join("greeting.txt"), "hello\n").expect("write");

    let mut index = repo.index().expect("index");
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .expect("add");
    index.write().expect("write index");
    let tree_oid = index.write_tree().expect("tree");
    let tree = repo.find_tree(tree_oid).expect("find tree");
    let signature = Signature::now("test", "test@example.com").expect("signature");
    repo.commit(Some("HEAD"), &signature, &signature, "commit", &tree, &[])
        .expect("commit");

    dir
}

#[test]
fn the_backend_reports_its_conflict_model_and_binding() {
    let dir = repo_with_commit();
    let vcs = GitVcs::open(dir.path()).expect("vcs");
    let capabilities = vcs.capabilities();

    assert_eq!(capabilities.conflict_model, ConflictModel::Blocking);
    assert!(!capabilities.op_log);
}

#[test]
fn prepare_materialise_snapshot_and_tip_round_trip() {
    let dir = repo_with_commit();
    let vcs = GitVcs::open(dir.path()).expect("vcs");

    let prepared = vcs.prepare("example").expect("prepare");
    assert_eq!(prepared.workspace.branch().as_str(), "litterbox/example");

    let staged = TempDir::new().expect("staging");
    vcs.materialise(&prepared.workspace, &prepared.base, staged.path())
        .expect("materialise");
    assert_eq!(
        fs::read_to_string(staged.path().join("greeting.txt")).expect("read"),
        "hello\n"
    );

    fs::write(staged.path().join("added.txt"), "added\n").expect("write");
    let revision = vcs
        .snapshot(&prepared.workspace, staged.path(), "snapshot")
        .expect("snapshot");
    let tip = vcs.tip(&prepared.workspace).expect("tip");

    assert_eq!(revision.as_str(), tip.as_str());
}

#[test]
fn delete_resolves_the_branch_and_a_missing_branch_is_not_found() {
    let dir = repo_with_commit();
    let vcs = GitVcs::open(dir.path()).expect("vcs");
    vcs.prepare("example").expect("prepare");

    assert_eq!(vcs.sandboxes().expect("sandboxes"), vec!["example"]);
    vcs.delete_branch("example").expect("delete");
    assert!(vcs.sandboxes().expect("sandboxes").is_empty());

    let error = vcs.delete_branch("example").expect_err("missing");
    assert!(matches!(error, Error::NotFound { .. }));
}

#[test]
fn the_unimplemented_surface_returns_the_neutral_error() {
    let dir = repo_with_commit();
    let vcs = GitVcs::open(dir.path()).expect("vcs");

    let error = vcs
        .merge(
            &Revision::new("a"),
            &Revision::new("b"),
            &Revision::new("c"),
        )
        .expect_err("merge is not implemented");

    match error {
        Error::Unsupported { operation, .. } => assert_eq!(operation, "merge"),
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn git_cannot_bind_a_host_directory() {
    let dir = repo_with_commit();
    let vcs = GitVcs::open(dir.path()).expect("vcs");
    let prepared = vcs.prepare("example").expect("prepare");

    let error = vcs
        .bound_root(&prepared.workspace)
        .expect_err("Git owns no host directory to bind");

    match error {
        Error::Unsupported {
            operation, backend, ..
        } => {
            assert_eq!(operation, "bind the workspace");
            assert_eq!(backend, "git");
        }
        other => panic!("unexpected error: {other}"),
    }
}

fn ref_oid(dir: &std::path::Path, name: &str) -> Option<String> {
    Repository::open(dir)
        .ok()?
        .refname_to_id(name)
        .ok()
        .map(|oid| oid.to_string())
}

#[test]
fn prepare_creates_the_branch_and_the_bookkeeping_refs() {
    let dir = repo_with_commit();
    let vcs = GitVcs::open(dir.path()).expect("vcs");
    let prepared = vcs.prepare("example").expect("prepare");

    let branch = ref_oid(dir.path(), "refs/heads/litterbox/example").expect("branch");
    assert_eq!(prepared.base.as_str(), branch);
    assert_eq!(
        ref_oid(dir.path(), "refs/litterbox/workspaces/example"),
        Some(branch.clone())
    );
    assert_eq!(
        ref_oid(dir.path(), "refs/litterbox/synced/example"),
        Some(branch)
    );
}

#[test]
fn delete_removes_the_branch_and_the_bookkeeping_refs() {
    let dir = repo_with_commit();
    let vcs = GitVcs::open(dir.path()).expect("vcs");
    vcs.prepare("example").expect("prepare");

    vcs.delete_branch("example").expect("delete");

    assert_eq!(ref_oid(dir.path(), "refs/heads/litterbox/example"), None);
    assert_eq!(
        ref_oid(dir.path(), "refs/litterbox/workspaces/example"),
        None
    );
    assert_eq!(ref_oid(dir.path(), "refs/litterbox/synced/example"), None);
    assert!(matches!(
        vcs.delete_branch("example").expect_err("missing"),
        Error::NotFound { .. }
    ));
}

#[test]
fn ensure_refs_recreates_a_legacy_records_bookkeeping_refs() {
    let dir = repo_with_commit();
    let vcs = GitVcs::open(dir.path()).expect("vcs");
    let prepared = vcs.prepare("example").expect("prepare");

    {
        let repo = Repository::open(dir.path()).expect("open");
        for name in [
            "refs/litterbox/workspaces/example",
            "refs/litterbox/synced/example",
        ] {
            repo.find_reference(name)
                .expect("ref")
                .delete()
                .expect("delete");
        }
    }

    vcs.ensure_refs(&prepared.workspace).expect("ensure");

    let branch = ref_oid(dir.path(), "refs/heads/litterbox/example").expect("branch");
    assert_eq!(
        ref_oid(dir.path(), "refs/litterbox/workspaces/example"),
        Some(branch.clone())
    );
    assert_eq!(
        ref_oid(dir.path(), "refs/litterbox/synced/example"),
        Some(branch)
    );
}

#[test]
fn preparing_an_existing_branch_reports_exists() {
    let dir = repo_with_commit();
    let vcs = GitVcs::open(dir.path()).expect("vcs");
    vcs.prepare("example").expect("first prepare");

    let error = vcs.prepare("example").expect_err("second prepare");

    assert!(matches!(error, Error::Exists { .. }));
}

#[test]
fn a_configured_store_slug_gives_the_backend_a_state_path() {
    let dir = repo_with_commit();
    let vcs = GitVcs::open(dir.path())
        .expect("vcs")
        .set_store_slug("project".to_string());

    assert!(vcs.state_path().is_some());
    assert_eq!(GitVcs::open(dir.path()).expect("vcs").state_path(), None);
}
