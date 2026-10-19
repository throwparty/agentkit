//! MCP spawn-hygiene tests: argv construction, env filtering, credential resolution, and the stderr relay.

use std::collections::BTreeMap;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

use super::*;

/// Records the spawn-hygiene calls in order, including the env
/// clear, so argv construction and env filtering are assertable.
#[derive(Default)]
struct RecordingCommand {
    program: Option<String>,
    args: Vec<String>,
    cleared: bool,
    env: BTreeMap<String, String>,
    env_set_after_clear: Vec<(String, String)>,
}

impl ConfigureCommand for RecordingCommand {
    fn set_program(&mut self, program: &str) {
        self.program = Some(program.to_owned());
    }

    fn set_args(&mut self, args: &[String]) {
        self.args = args.to_vec();
    }

    fn clear_env(&mut self) {
        self.cleared = true;
    }

    fn set_env(&mut self, key: &str, value: &str) {
        self.env.insert(key.to_owned(), value.to_owned());
        self.env_set_after_clear
            .push((key.to_owned(), value.to_owned()));
    }
}

#[test]
fn argv_construction_is_explicit_without_a_shell() {
    let mut cmd = RecordingCommand::default();
    configure_stdio_spawn(
        &mut cmd,
        "mcp-server-everything",
        &["--verbose".into()],
        &BTreeMap::new(),
        "keychain",
    )
    .unwrap();
    assert_eq!(cmd.program.as_deref(), Some("mcp-server-everything"));
    assert_eq!(cmd.args, vec!["--verbose".to_owned()]);
    // No shell: the program name is the argv[0] verbatim, never a
    // shell command line.
    assert!(!cmd.program.as_deref().unwrap_or_default().contains(' '));
}

#[test]
fn env_is_cleared_then_only_named_entries_set() {
    let mut env = BTreeMap::new();
    env.insert("MCP_TOKEN".to_owned(), "secret".to_owned());
    env.insert("PATH".to_owned(), "/custom/bin".to_owned());

    let mut cmd = RecordingCommand::default();
    configure_stdio_spawn(&mut cmd, "server", &[], &env, "keychain").unwrap();

    assert!(cmd.cleared, "no blanket environment forwarding");
    assert_eq!(cmd.env, env);
    // Entries are set only after the clear, so the parent's env is
    // never forwarded.
    assert_eq!(cmd.env_set_after_clear.len(), 2);
}

/// Writes an `agentkit-credential-test` helper that rejects any
/// invocation missing the `get tackle <identity>` shape, like the
/// shipped helpers do.
fn write_fake_helper(dir: &std::path::Path, body: &str) {
    let path = dir.join("agentkit-credential-test");
    let mut file = std::fs::File::create(&path).unwrap();
    use std::io::Write as _;
    writeln!(
        file,
        "#!/bin/sh\n\
             [ \"$#\" -eq 3 ] && [ \"$1\" = get ] && [ \"$2\" = tackle ] || exit 3\n\
             echo '{body}'"
    )
    .unwrap();
    file.flush().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn cred_markers_resolve_through_the_credential_helper() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_helper(dir.path(), "{\"access_token\": \"tok\"}");

    let mut env = BTreeMap::new();
    env.insert("GITHUB_TOKEN".to_owned(), "{cred:github}".to_owned());
    env.insert("LITERAL".to_owned(), "plain-value".to_owned());

    let mut cmd = RecordingCommand::default();
    let _guard = crate::credentials::PATH_LOCK.lock().unwrap();
    let path_var = std::env::var("PATH").unwrap();
    std::env::set_var("PATH", format!("{}:{}", dir.path().display(), path_var));
    let resolved = configure_stdio_spawn(&mut cmd, "server", &[], &env, "test");
    std::env::set_var("PATH", path_var);

    resolved.unwrap();
    assert_eq!(cmd.env.get("GITHUB_TOKEN").map(String::as_str), Some("tok"),);
    assert_eq!(
        cmd.env.get("LITERAL").map(String::as_str),
        Some("plain-value"),
    );
}

#[test]
fn a_failed_cred_resolution_leaves_the_command_untouched() {
    let mut env = BTreeMap::new();
    env.insert("GITHUB_TOKEN".to_owned(), "{cred:github}".to_owned());

    let mut cmd = RecordingCommand::default();
    let err = configure_stdio_spawn(&mut cmd, "server", &[], &env, "definitely-missing-helper")
        .unwrap_err();

    assert!(err.to_string().contains("GITHUB_TOKEN"), "{err}");
    // Resolution happens before any mutation: a failure never
    // half-spawns with an unresolved secret.
    assert_eq!(cmd.program, None, "the command was not configured");
    assert!(cmd.env.is_empty());
}

#[test]
fn an_empty_cred_identity_is_rejected() {
    let mut env = BTreeMap::new();
    env.insert("GITHUB_TOKEN".to_owned(), "{cred:}".to_owned());

    let mut cmd = RecordingCommand::default();
    let err = configure_stdio_spawn(&mut cmd, "server", &[], &env, "test").unwrap_err();
    assert!(err.to_string().contains("empty identity"), "{err}");
}

#[test]
fn a_cred_marker_must_be_the_entire_value() {
    let mut env = BTreeMap::new();
    env.insert("AUTH".to_owned(), "Bearer {cred:github}".to_owned());

    let mut cmd = RecordingCommand::default();
    let err = configure_stdio_spawn(&mut cmd, "server", &[], &env, "test").unwrap_err();
    assert!(err.to_string().contains("entire value"), "{err}");
}

#[tokio::test]
async fn stderr_relay_prefixes_lines_with_the_server_name() {
    let (mut client, server) = tokio::io::duplex(64);
    client.write_all(b"hello\nworld\n").await.unwrap();
    drop(client);

    let mut out = Vec::new();
    let mut cap = StderrRateCap::new(STDERR_RELAY_MAX_LINES, STDERR_RELAY_WINDOW);
    relay_stderr("echo", server, &mut out, &mut cap)
        .await
        .unwrap();

    let text = String::from_utf8(out).unwrap();
    assert_eq!(text, "mcp[echo] hello\nmcp[echo] world\n");
}

#[tokio::test]
async fn stderr_relay_is_rate_capped_within_a_window() {
    let (mut client, server) = tokio::io::duplex(512);
    for i in 0..20 {
        client
            .write_all(format!("line {i}\n").as_bytes())
            .await
            .unwrap();
    }
    drop(client);

    let mut out = Vec::new();
    // A fresh cap in a young window: at most 5 lines survive.
    let mut cap = StderrRateCap::new(5, STDERR_RELAY_WINDOW);
    relay_stderr("echo", server, &mut out, &mut cap)
        .await
        .unwrap();

    let text = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 5, "only the capped number of lines relayed");
    assert!(lines.iter().all(|line| line.starts_with("mcp[echo] ")));
}

#[tokio::test]
async fn stderr_relay_truncates_an_overlong_line_and_keeps_going() {
    let limit = super::stderr::STDERR_RELAY_MAX_LINE_BYTES;
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let mut data = vec![b'x'; limit + 512];
    data.extend_from_slice(b"\ntail\n");
    client.write_all(&data).await.unwrap();
    drop(client);

    let mut out = Vec::new();
    let mut cap = StderrRateCap::new(STDERR_RELAY_MAX_LINES, STDERR_RELAY_WINDOW);
    relay_stderr("echo", server, &mut out, &mut cap)
        .await
        .unwrap();

    let text = String::from_utf8(out).unwrap();
    let mut lines = text.lines();
    let first = lines.next().unwrap();
    assert!(first.starts_with("mcp[echo] xxx"), "{first:?}");
    assert!(first.ends_with(" [truncated]"), "{first:?}");
    assert_eq!(
        first.len(),
        "mcp[echo] ".len() + limit + " [truncated]".len()
    );
    assert_eq!(lines.next(), Some("mcp[echo] tail"));
    assert_eq!(lines.next(), None);
}

#[test]
fn rate_cap_reports_dropped_lines_on_window_reset() {
    let start = std::time::Instant::now();
    let mut cap = StderrRateCap::new(2, Duration::from_secs(10));

    assert_eq!(cap.admit(start), Admission::Relay);
    assert_eq!(cap.admit(start), Admission::Relay);
    assert_eq!(cap.admit(start), Admission::Drop);
    assert_eq!(cap.admit(start), Admission::Drop);

    // After the window rolls over the cap lifts and the dropped
    // count is reported once.
    let later = start + Duration::from_secs(11);
    assert_eq!(cap.admit(later), Admission::WindowReset { dropped: 2 });
    assert_eq!(cap.admit(later), Admission::Relay);
}

#[test]
fn credential_marker_identity_only_accepts_the_whole_marker() {
    assert_eq!(credential_marker_identity("{cred:github}"), Some("github"));
    assert_eq!(credential_marker_identity("Bearer {cred:github}"), None);
    assert_eq!(credential_marker_identity("{cred:}"), None);
    assert_eq!(credential_marker_identity("{cred:github"), None);
    assert_eq!(credential_marker_identity("plain"), None);
}
