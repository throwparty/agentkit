//! The doctor entry point does not mutate the repository it inspects.
#![allow(clippy::await_holding_lock)]

use std::path::Path;

static CWD: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn repository() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    git2::Repository::init(dir.path()).expect("repo");
    let config = dir.path().join(".agentkit/litterbox/config.toml");
    std::fs::create_dir_all(config.parent().expect("parent")).expect("dirs");
    std::fs::write(&config, "[project]\nbackends = [\"docker\"]\n").expect("config");
    dir
}

fn branches(dir: &Path) -> usize {
    git2::Repository::open(dir)
        .expect("repo")
        .branches(None)
        .expect("branches")
        .count()
}

#[tokio::test]
async fn a_misconfigured_doctor_run_creates_no_branch() {
    let _guard = CWD.lock().expect("cwd lock");
    let dir = repository();
    let config = dir.path().join(".agentkit/litterbox/config.toml");
    std::fs::write(&config, "[project]\nsetup-command = 'echo hi'\n").expect("config");
    let before = branches(dir.path());
    std::env::set_current_dir(dir.path()).expect("cwd");

    let _ = agentkit_litterbox::cli::handle_doctor(None, false).await;

    assert_eq!(branches(dir.path()), before, "the doctor created a branch");
}

#[tokio::test]
async fn a_ready_doctor_run_creates_no_branch() {
    let _guard = CWD.lock().expect("cwd lock");
    let dir = repository();
    let before = branches(dir.path());
    std::env::set_current_dir(dir.path()).expect("cwd");

    let _ = agentkit_litterbox::cli::handle_doctor(None, false).await;

    assert_eq!(branches(dir.path()), before, "the doctor created a branch");
}
