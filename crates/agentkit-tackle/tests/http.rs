//! The HTTP transport's integration tests (T-036): initialize through
//! the session lifecycle over HTTP; multiple concurrent connections
//! served by one process.

use serde_json::json;
use std::process::{Child, Command, Stdio};

struct HttpAgent {
    child: Child,
    port: u16,
}

impl Drop for HttpAgent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_http_agent(dir: &std::path::Path, name: &str) -> HttpAgent {
    std::fs::create_dir_all(dir.join(name)).unwrap();
    // Pick a free port by binding one first.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let child = Command::new(env!("CARGO_BIN_EXE_agentkit-tackle"))
        .args([
            "http",
            "--bind",
            "127.0.0.1",
            "--http-port",
            &port.to_string(),
            "--config-dir",
            dir.join(name).to_str().unwrap(),
            "--db-path",
            dir.join("sessions.db").to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    HttpAgent { child, port }
}

/// POSTs one JSON-RPC message; the optional connection header continues
/// a connection.
fn rpc(
    agent: &HttpAgent,
    connection: Option<&str>,
    message: serde_json::Value,
) -> (serde_json::Value, Option<String>) {
    let url = match connection {
        Some(connection) => format!(
            "http://127.0.0.1:{}/rpc?connection={connection}",
            agent.port
        ),
        None => format!("http://127.0.0.1:{}/rpc", agent.port),
    };
    let response = reqwest::blocking::Client::new()
        .post(url)
        .header("content-type", "application/json")
        .body(message.to_string())
        .send()
        .unwrap();
    let header = response
        .headers()
        .get("x-acp-connection")
        .and_then(|value| value.to_str().ok().map(str::to_owned));
    let body = response.text().unwrap();
    (serde_json::from_str(&body).unwrap_or(json!({})), header)
}

fn initialize() -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": 1, "clientCapabilities": {} }
    })
}

fn wait_for_port(agent: &HttpAgent) {
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", agent.port)).is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("the HTTP agent never listened");
}

#[test]
fn initialize_through_session_lifecycle_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let agent = spawn_http_agent(dir.path(), "a");
    wait_for_port(&agent);

    // initialize: opens the connection; the id comes back in a header.
    let (response, connection) = rpc(&agent, None, initialize());
    assert_eq!(response["result"]["protocolVersion"], 1, "{response}");
    let connection = connection.expect("the connection id in the response header");

    // session/new over the same connection.
    let (response, _) = rpc(
        &agent,
        Some(&connection),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/new",
            "params": { "cwd": "/http-work", "mcpServers": [] }
        }),
    );
    let session_id = response["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_string();

    // session/list sees it.
    let (response, _) = rpc(
        &agent,
        Some(&connection),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/list",
            "params": {}
        }),
    );
    let sessions: Vec<&str> = response["result"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|info| info["sessionId"].as_str().unwrap())
        .collect();
    assert_eq!(sessions, vec![session_id.as_str()], "{response}");

    // session/close: lifecycle completes.
    let (response, _) = rpc(
        &agent,
        Some(&connection),
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "session/close",
            "params": { "sessionId": session_id }
        }),
    );
    assert!(response.get("result").is_some(), "{response}");
}

#[test]
fn multiple_concurrent_connections_are_served() {
    let dir = tempfile::tempdir().unwrap();
    let agent = spawn_http_agent(dir.path(), "a");
    wait_for_port(&agent);

    // Two independent clients initialize concurrently: each gets its
    // own connection id.
    let (first, second) = std::thread::scope(|scope| {
        let handle_a = scope.spawn(|| rpc(&agent, None, initialize()));
        let handle_b = scope.spawn(|| rpc(&agent, None, initialize()));
        (handle_a.join().unwrap(), handle_b.join().unwrap())
    });
    let connection_a = first.1.expect("first connection id");
    let connection_b = second.1.expect("second connection id");
    assert_ne!(connection_a, connection_b);

    // Both connections drive their own sessions.
    for (connection, id) in [(connection_a.clone(), 10), (connection_b, 20)] {
        let (response, _) = rpc(
            &agent,
            Some(connection.as_str()),
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "session/new",
                "params": { "cwd": "/concurrent", "mcpServers": [] }
            }),
        );
        assert!(response["result"]["sessionId"].is_string(), "{response}");
    }

    // Both sessions are listable.
    let (response, _) = rpc(
        &agent,
        Some(connection_a.as_str()),
        json!({
            "jsonrpc": "2.0",
            "id": 30,
            "method": "session/list",
            "params": {}
        }),
    );
    let sessions = response["result"]["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
}
