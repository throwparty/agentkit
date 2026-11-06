//! The doctor entry point does not mutate the repository it inspects.
#![allow(clippy::await_holding_lock)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

static CWD: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn repository() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    git2::Repository::init(dir.path()).expect("repo");
    let config = dir.path().join(".agentkit/litterbox/config.toml");
    std::fs::create_dir_all(config.parent().expect("parent")).expect("dirs");
    std::fs::write(
        &config,
        "[project]\nbackends = [\"docker\"]\n\
         [[provision]]\nidentity = \"fetch\"\nkind = \"command\"\nrun = \"true\"\n\
         [backends.docker]\nprovision = [\"fetch\"]\n",
    )
    .expect("config");
    dir
}

fn branches(dir: &Path) -> usize {
    git2::Repository::open(dir)
        .expect("repo")
        .branches(None)
        .expect("branches")
        .count()
}

/// Everything under `dir`, with file content hashes, so a record, workspace or
/// rewritten file written by the doctor is visible even when no branch was
/// created. Directories are recorded as `None`.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Option<u64>)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(PathBuf, Option<u64>)>) {
        for entry in std::fs::read_dir(dir).expect("read_dir") {
            let entry = entry.expect("entry");
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            let metadata = entry.metadata().expect("metadata");
            let relative = path.strip_prefix(root).expect("relative").to_path_buf();
            if metadata.is_dir() {
                out.push((relative, None));
                walk(&path, root, out);
            } else {
                let contents = std::fs::read(&path).expect("read");
                let mut hasher = DefaultHasher::new();
                contents.hash(&mut hasher);
                out.push((relative, Some(hasher.finish())));
            }
        }
    }
    if !dir.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

#[tokio::test]
async fn a_misconfigured_doctor_run_creates_no_branch() {
    let _guard = CWD.lock().expect("cwd lock");
    let dir = repository();
    let config = dir.path().join(".agentkit/litterbox/config.toml");
    std::fs::write(&config, "[project]\nsetup-command = 'echo hi'\n").expect("config");
    let before_branches = branches(dir.path());
    let before_files = snapshot(dir.path());
    std::env::set_current_dir(dir.path()).expect("cwd");

    let _ = agentkit_litterbox::cli::handle_doctor(None, false).await;

    assert_eq!(
        branches(dir.path()),
        before_branches,
        "the doctor created a branch"
    );
    assert_eq!(
        snapshot(dir.path()),
        before_files,
        "the doctor wrote a file into the checkout"
    );
}

#[tokio::test]
async fn a_provisioned_doctor_run_creates_no_branch_or_record() {
    let _guard = CWD.lock().expect("cwd lock");
    let dir = repository();
    std::env::set_current_dir(dir.path()).expect("cwd");
    let records = agentkit_litterbox::metadata_store::directory("");

    let before_branches = branches(dir.path());
    let before_files = snapshot(dir.path());
    let before_records = snapshot(&records);

    let _ = agentkit_litterbox::cli::handle_doctor(None, false).await;

    assert_eq!(
        branches(dir.path()),
        before_branches,
        "the doctor created a branch"
    );
    assert_eq!(
        snapshot(dir.path()),
        before_files,
        "the doctor wrote a file into the checkout"
    );
    assert_eq!(
        snapshot(&records),
        before_records,
        "the doctor wrote to the metadata store"
    );
}
