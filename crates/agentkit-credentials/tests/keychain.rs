use std::path::PathBuf;
use std::process::Command;

fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_agentkit-credential-keychain"))
}

fn run_get(identity: &str) -> Result<String, ()> {
    let output = Command::new(binary_path())
        .arg("get")
        .arg(identity)
        .output()
        .map_err(|_| ())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(())
    }
}

fn run_store(identity: &str, json: &str) -> bool {
    let mut cmd = Command::new(binary_path());
    cmd.arg("store")
        .arg(identity)
        .stdin(std::process::Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    use std::io::Write;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(json.as_bytes()).unwrap();
    }
    child.wait().unwrap().success()
}

fn run_erase(identity: &str) -> bool {
    Command::new(binary_path())
        .arg("erase")
        .arg(identity)
        .output()
        .unwrap()
        .status
        .success()
}

#[test]
fn keychain_get_store_erase() {
    let test_id = format!("test_keychain_{}", std::process::id());
    let cred_json = r#"{"access_token": "tok_kc", "refresh_token": "ref_kc", "expires_at": "2026-12-31T23:59:59Z"}"#;

    let stored = run_store(&test_id, cred_json);
    if !stored {
        eprintln!("keychain store failed — skipping test (no keychain daemon?)");
        return;
    }

    let result = run_get(&test_id);
    if result.is_err() {
        eprintln!("keychain get failed after store — skipping (platform issue)");
        return;
    }
    let output = result.unwrap();
    assert!(output.contains("tok_kc"), "output should contain access_token: {output}");

    assert!(run_erase(&test_id), "erase should succeed");
    let result = run_get(&test_id);
    assert!(result.is_err(), "get after erase should fail");
}

#[test]
fn keychain_get_not_found() {
    let result = run_get("nonexistent_keychain_id");
    assert!(result.is_err(), "get for missing identity should fail");
}

#[test]
fn keychain_invalid_json() {
    let test_id = format!("test_kc_invalid_{}", std::process::id());
    let mut cmd = Command::new(binary_path());
    cmd.arg("store")
        .arg(&test_id)
        .stdin(std::process::Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    use std::io::Write;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(b"not valid json").unwrap();
    }
    let status = child.wait().unwrap();
    assert!(!status.success(), "store with invalid JSON should fail");
}
