//! Golden-transcript conformance harness: replay recorded client→agent
//! JSON-RPC transcripts against the tackle binary and subset-match the
//! responses.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// A spawned tackle process speaking raw JSON-RPC on stdio.
struct Agent {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: u64,
}

impl Agent {
    fn spawn(dir: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agentkit-tackle"))
            .arg("--config-dir")
            .arg(dir.join("cfg"))
            .arg("--db-path")
            .arg(dir.join("sessions.db"))
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn tackle");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: 0,
        }
    }

    /// Sends a request and reads lines until the response with this id
    /// arrives; every interim line must parse as valid JSON.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{line}").expect("write request");

        loop {
            let mut line = String::new();
            let read = self.reader.read_line(&mut line).expect("read agent output");
            assert!(
                read > 0,
                "agent closed stdout before responding to {method}"
            );
            let value: Value = serde_json::from_str(&line).expect("agent line is valid JSON");
            if value.get("id").and_then(Value::as_u64) == Some(id) && !value.get("method").is_some()
            {
                return value;
            }
            // Interim notification: tolerated (the transcript's `expect`
            // assertions cover semantic content separately).
        }
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Recursive subset match: every key/index in `expected` must exist in
/// `actual` with an equal value; extra keys in `actual` are ignored.
fn assert_subset(expected: &Value, actual: &Value, path: &str) {
    match (expected, actual) {
        (Value::Object(expected_map), Value::Object(actual_map)) => {
            for (key, expected_value) in expected_map {
                let actual_value = actual_map
                    .get(key)
                    .unwrap_or_else(|| panic!("{path}.{key}: missing — actual: {actual}"));
                assert_subset(expected_value, actual_value, &format!("{path}.{key}"));
            }
        }
        (Value::Array(expected_items), Value::Array(actual_items)) => {
            assert_eq!(
                actual_items.len(),
                expected_items.len(),
                "{path}: array length"
            );
            for (index, (expected, actual)) in
                expected_items.iter().zip(actual_items.iter()).enumerate()
            {
                assert_subset(expected, actual, &format!("{path}[{index}]"));
            }
        }
        (Value::Null, _) => {
            assert!(actual.is_null(), "{path}: expected null, got {actual}");
        }
        (Value::String(expected), _) if expected.ends_with('*') => {
            let prefix = expected.trim_end_matches('*');
            assert!(
                actual
                    .as_str()
                    .is_some_and(|actual| actual.starts_with(prefix)),
                "{path}: expected a string starting with {prefix:?}, got {actual}"
            );
        }
        (expected, actual) => {
            assert_eq!(expected, actual, "{path}: scalar mismatch");
        }
    }
}

/// Runs one transcript fixture: a sequence of (method, params, expect)
/// steps against a fresh agent. Params may reference `"$sessionId"` —
/// substituted with the session id captured from the most recent
/// session/new response.
fn run_transcript(name: &str, transcript: &Value) {
    let dir = tempfile::tempdir().unwrap();
    let mut agent = Agent::spawn(dir.path());

    let steps = transcript["steps"].as_array().expect("steps array");
    let mut session_id: Option<String> = None;
    for (index, step) in steps.iter().enumerate() {
        let method = step["method"].as_str().expect("method");
        let mut params = step.get("params").cloned().unwrap_or(json!({}));
        if let Some(captured) = &session_id {
            params = substitute_session_id(&params, captured);
        }
        let response = agent.request(method, params);
        if let Some(expected) = step.get("expect") {
            assert_subset(
                expected,
                &response,
                &format!("{name} step {index} ({method})"),
            );
        }
        if let Some(captured) = response["result"]["sessionId"].as_str() {
            session_id = Some(captured.to_owned());
        }
    }
}

/// Deep-replaces the string `"$sessionId"` in a params value.
fn substitute_session_id(value: &Value, session_id: &str) -> Value {
    match value {
        Value::String(text) if text == "$sessionId" => Value::String(session_id.to_owned()),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| substitute_session_id(item, session_id))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), substitute_session_id(value, session_id)))
                .collect(),
        ),
        value => value.clone(),
    }
}

const LIFECYCLE: &str = include_str!("transcripts/lifecycle.json");

#[test]
fn lifecycle_transcript_conforms() {
    let transcript: Value = serde_json::from_str(LIFECYCLE).unwrap();
    run_transcript("lifecycle", &transcript);
}
