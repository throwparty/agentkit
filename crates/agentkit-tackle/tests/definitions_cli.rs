//! The definitions subcommand resolves its project directory under the
//! repository root, not the process working directory.

use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_agentkit-tackle"))
        .args(args)
        .output()
        .expect("run tackle")
}

#[test]
fn definitions_are_resolved_under_the_repository_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_str().unwrap().to_owned();

    let output = run(&["definitions", "--project-dir", &root, "init"]);
    assert!(output.status.success(), "{output:?}");
    let manifest = dir
        .path()
        .join(".agentkit")
        .join("tackle")
        .join("definitions.toml");
    assert!(manifest.is_file(), "manifest at {}", manifest.display());

    let output = run(&["def", "--project-dir", &root, "list"]);
    assert!(output.status.success(), "{output:?}");
}
