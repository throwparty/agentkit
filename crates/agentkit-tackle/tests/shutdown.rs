//! Graceful shutdown (cli.mdx "Shutdown"): the process ends cleanly on
//! SIGTERM rather than being killed. Before the fix `wait_for_shutdown`
//! was never awaited, so the handler was dead code.

#[cfg(unix)]
#[test]
fn sigterm_shuts_the_stdio_process_down_cleanly() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let dir = tempfile::tempdir().unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_agentkit-tackle"))
        .current_dir(dir.path())
        .args([
            "stdio",
            "--config-dir",
            dir.path().join("cfg").to_str().unwrap(),
            "--db-path",
            dir.path().join("sessions.db").to_str().unwrap(),
        ])
        // stdin stays open: closing it would end the transport before the
        // signal arrives.
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn tackle");

    // Let the runtime install its signal handlers.
    std::thread::sleep(Duration::from_millis(500));

    let killed = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("send SIGTERM");
    assert!(killed.success(), "kill delivered SIGTERM");

    let output = child.wait_with_output().expect("wait for tackle");
    assert!(
        output.status.success(),
        "a graceful shutdown exits successfully: {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("shutdown complete"), "{stderr}");
}
