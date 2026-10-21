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

#[test]
fn get_without_a_lock_vendors_from_a_path_source() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_str().unwrap().to_owned();
    let shared = dir.path().join("shared");
    std::fs::create_dir_all(shared.join("prompts").join("triage")).unwrap();
    std::fs::write(
        shared.join("prompts/triage/PROMPT.md"),
        "---\ndescription: Triage\n---\nTriage it.\n",
    )
    .unwrap();

    let output = run(&["definitions", "--project-dir", &root, "init"]);
    assert!(output.status.success(), "{output:?}");
    std::fs::write(
        dir.path().join(".agentkit/tackle/definitions.toml"),
        r#"
[[source]]
name = "local"
kind = "path"

[source.path]
dir = "shared"

[definitions.local]
prompts = ["triage"]
"#,
    )
    .unwrap();

    let output = run(&["def", "--project-dir", &root, "get"]);
    assert!(output.status.success(), "{output:?}");
    let vendored = dir.path().join(".agentkit/tackle/prompts/triage/PROMPT.md");
    assert!(vendored.is_file(), "vendored at {}", vendored.display());
    assert!(dir
        .path()
        .join(".agentkit/tackle/definitions.lock")
        .is_file());
}
