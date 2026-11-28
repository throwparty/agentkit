//! Behaviour tests for the host commands.

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

use crate::vcs::{Revision, Vcs};

use super::GitVcs;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
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

fn git_output(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .args(args)
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn repository() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    git(dir.path(), &["init", "-q", "-b", "main"]);
    fs::write(dir.path().join("file.txt"), "base\n").expect("write");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "base"]);
    dir
}

/// A synced base, a sandbox change and a user rewrite that conflicts with it.
fn diverged() -> (TempDir, TempDir, GitVcs, crate::vcs::Workspace) {
    let dir = repository();
    let root = TempDir::new().expect("reconcile root");
    let vcs = GitVcs::open(dir.path())
        .expect("vcs")
        .set_store_slug("project".to_string())
        .with_reconcile_root(root.path().to_path_buf());
    let prepared = vcs.prepare("example").expect("prepare");

    let staging = TempDir::new().expect("staging");
    vcs.materialise(&prepared.workspace, &prepared.base, staging.path())
        .expect("materialise");
    fs::write(staging.path().join("file.txt"), "sandbox\n").expect("write");
    vcs.snapshot(&prepared.workspace, staging.path(), "snapshot")
        .expect("snapshot");

    // `synced` stays at the base, as it does while a divergence is held.
    vcs.set_synced(&prepared.workspace, &prepared.base)
        .expect("synced");

    git(dir.path(), &["checkout", "-q", "-b", "user"]);
    fs::write(dir.path().join("file.txt"), "user\n").expect("write");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "rewrite"]);
    let rewritten = git_output(dir.path(), &["rev-parse", "HEAD"]);
    git(
        dir.path(),
        &["update-ref", "refs/heads/litterbox/example", &rewritten],
    );

    (dir, root, vcs, prepared.workspace)
}

#[test]
fn a_branch_checked_out_in_a_worktree_refuses_restore() {
    let dir = repository();
    git(dir.path(), &["branch", "litterbox/example"]);
    let linked = TempDir::new().expect("linked");
    let worktree = linked.path().join("worktree");
    git(
        dir.path(),
        &[
            "worktree",
            "add",
            worktree.to_str().expect("utf8"),
            "litterbox/example",
        ],
    );

    let vcs = GitVcs::open(dir.path()).expect("vcs");
    let workspace = crate::vcs::Workspace::from_branch("litterbox/example");
    let target = Revision::new(git_output(dir.path(), &["rev-parse", "HEAD"]));
    let error = vcs
        .restore_preflight(&workspace, &target)
        .expect_err("the branch is checked out");

    let message = error.to_string();
    assert!(message.contains("checked out"), "{message}");
    assert!(
        message.contains(worktree.to_str().expect("utf8")),
        "the refusal names the worktree: {message}"
    );
}

#[test]
fn a_marker_file_reports_a_base_when_the_path_is_in_the_base() {
    let dir = repository();
    fs::write(dir.path().join("conflicted.txt"), "base\n").expect("write");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "base"]);
    let base = git_output(dir.path(), &["rev-parse", "HEAD"]);

    fs::write(
        dir.path().join("conflicted.txt"),
        "<<<<<<< x\nours\n=======\ntheirs\n>>>>>>> y\n",
    )
    .expect("write");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "conflict"]);

    let conflicts =
        super::reconcile::marker_conflicts(dir.path(), "HEAD", Some(&base)).expect("scan");
    assert_eq!(conflicts.len(), 1, "{conflicts:?}");
    assert!(conflicts[0].base, "the path exists in the recorded base");
}

#[test]
fn a_marker_file_with_a_colon_in_its_name_is_reported() {
    let dir = repository();
    fs::write(dir.path().join("a:b.txt"), "<<<<<<< x\nours\n").expect("write");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "conflict"]);

    let conflicts = super::reconcile::marker_conflicts(dir.path(), "HEAD", None).expect("scan");
    assert_eq!(conflicts.len(), 1, "{conflicts:?}");
    assert_eq!(conflicts[0].path, Path::new("a:b.txt"));
}

#[test]
fn the_report_finds_the_conflict_with_the_recorded_base() {
    let (dir, _root, vcs, workspace) = diverged();

    let report = vcs.report(&workspace).expect("report");

    assert_eq!(report.conflicts, vec![std::path::PathBuf::from("file.txt")]);
    assert!(report.reflog.is_some());
    assert!(report.worktree.is_none());
    assert_eq!(
        report.base.as_str(),
        git_output(dir.path(), &["rev-parse", "refs/litterbox/synced/example"])
    );
}

#[test]
fn staging_leaves_a_conflicted_worktree_and_the_checkout_alone() {
    let (dir, root, vcs, workspace) = diverged();
    let checkout_before = fs::read_to_string(dir.path().join("file.txt")).expect("read");

    let staged = vcs.stage(&workspace).expect("stage");

    assert_eq!(staged.conflicts, vec![std::path::PathBuf::from("file.txt")]);
    assert_eq!(staged.path, root.path().join("project/example"));
    assert!(staged.path.is_dir());
    let conflict = fs::read_to_string(staged.path.join("file.txt")).expect("read");
    assert!(conflict.contains("<<<<<<<"), "{conflict}");
    assert!(conflict.contains("======="), "{conflict}");
    assert!(conflict.contains(">>>>>>>"), "{conflict}");
    assert!(conflict.contains("user"), "{conflict}");
    assert!(conflict.contains("sandbox"), "{conflict}");

    let status = git_output(&staged.path, &["status", "--porcelain"]);
    assert!(status.contains("UU file.txt"), "{status}");
    let merge_head = git_output(&staged.path, &["rev-parse", "--git-path", "MERGE_HEAD"]);
    let merge_head = if Path::new(&merge_head).is_absolute() {
        std::path::PathBuf::from(merge_head)
    } else {
        staged.path.join(merge_head)
    };
    assert!(merge_head.exists(), "MERGE_HEAD is written");

    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).expect("read"),
        checkout_before,
        "the user's checkout is untouched"
    );

    let again = vcs.stage(&workspace).expect_err("second stage");
    assert!(again.to_string().contains("already exists"), "{again}");
}

#[test]
fn finish_lands_the_merge_and_removes_the_worktree() {
    let (dir, _root, vcs, workspace) = diverged();
    let staged = vcs.stage(&workspace).expect("stage");

    let unfinished = vcs.finish(&workspace).expect_err("unresolved");
    assert!(
        unfinished.to_string().contains("uncommitted"),
        "{unfinished}"
    );

    fs::write(staged.path.join("file.txt"), "resolved\n").expect("write");
    git(&staged.path, &["add", "file.txt"]);
    git(&staged.path, &["commit", "-qm", "resolve"]);
    let merge = git_output(&staged.path, &["rev-parse", "HEAD"]);

    let finished = vcs.finish(&workspace).expect("finish");
    match finished {
        crate::vcs::Finished::Landed { branch } => assert_eq!(branch.as_str(), merge),
        other => panic!("expected a landing, got {other:?}"),
    }

    assert_eq!(
        vcs.tip(&workspace).expect("tip").as_str(),
        merge,
        "the branch is fast-forwarded"
    );
    assert!(!staged.path.exists(), "the worktree is removed");
    assert!(!staged.path.exists());
    assert!(
        !git_output(
            dir.path(),
            &["branch", "--list", "litterbox-reconcile/example"]
        )
        .contains("litterbox-reconcile/example")
    );
}

#[test]
fn discard_removes_the_worktree_and_leaves_the_branch() {
    let (dir, _root, vcs, workspace) = diverged();
    let before = vcs.tip(&workspace).expect("tip");
    let staged = vcs.stage(&workspace).expect("stage");

    vcs.discard_stage(&workspace).expect("discard");

    assert!(!staged.path.exists());
    assert_eq!(
        vcs.tip(&workspace).expect("tip").as_str(),
        before.as_str(),
        "the branch is untouched"
    );
    assert!(
        !git_output(
            dir.path(),
            &["branch", "--list", "litterbox-reconcile/example"]
        )
        .contains("litterbox-reconcile/example")
    );
}
