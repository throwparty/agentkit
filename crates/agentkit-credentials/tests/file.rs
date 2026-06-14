use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

const COMPONENT: &str = "test";

fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_agentkit-credential-file"))
}

fn test_dir() -> PathBuf {
    let count = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("agentkit-cred-file-test-{}-{}", std::process::id(), count))
}

fn run_get(home: &PathBuf, identity: &str) -> (String, bool) {
    let output = Command::new(binary_path())
        .arg("get")
        .arg(COMPONENT)
        .arg(identity)
        .env("HOME", home)
        .output()
        .expect("binary should run");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    (stdout, output.status.success())
}

fn run_put(home: &PathBuf, identity: &str, json: &str) -> bool {
    let mut cmd = Command::new(binary_path());
    cmd.arg("put")
        .arg(COMPONENT)
        .arg(identity)
        .env("HOME", home)
        .stdin(std::process::Stdio::piped());
    let mut child = cmd.spawn().expect("binary should spawn");
    use std::io::Write;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(json.as_bytes()).unwrap();
    }
    child.wait().unwrap().success()
}

fn run_delete(home: &PathBuf, identity: &str) -> bool {
    Command::new(binary_path())
        .arg("delete")
        .arg(COMPONENT)
        .arg(identity)
        .env("HOME", home)
        .output()
        .unwrap()
        .status
        .success()
}

#[test]
fn file_get_put_delete() {
    let dir = test_dir();
    std::fs::create_dir_all(&dir).unwrap();

    let cred_json = r#"{"access_token": "tok_test", "refresh_token": "ref_test", "expires_at": "2026-12-31T23:59:59Z"}"#;

    assert!(run_put(&dir, "test_id", cred_json), "put should succeed");
    let (output, success) = run_get(&dir, "test_id");
    assert!(success, "get should succeed");
    assert!(output.contains("tok_test"), "output should contain access_token: {output}");
    assert!(output.contains("ref_test"), "output should contain refresh_token: {output}");

    assert!(run_delete(&dir, "test_id"), "delete should succeed");
    let (_, success) = run_get(&dir, "test_id");
    assert!(!success, "get after delete should fail");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn file_get_not_found() {
    let dir = test_dir();
    std::fs::create_dir_all(&dir).unwrap();

    let (_, success) = run_get(&dir, "nonexistent");
    assert!(!success, "get for missing identity should fail");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn file_invalid_json() {
    let dir = test_dir();
    std::fs::create_dir_all(&dir).unwrap();

    let mut cmd = Command::new(binary_path());
    cmd.arg("put")
        .arg(COMPONENT)
        .arg("test_id")
        .env("HOME", &dir)
        .stdin(std::process::Stdio::piped());
    let mut child = cmd.spawn().expect("binary should spawn");
    use std::io::Write;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(b"not valid json").unwrap();
    }
    let status = child.wait().unwrap();
    assert!(!status.success(), "put with invalid JSON should fail");

    let _ = std::fs::remove_dir_all(&dir);
}
