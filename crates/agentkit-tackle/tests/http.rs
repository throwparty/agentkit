//! The HTTP transport's integration tests: initialize through
//! the session lifecycle over HTTP; multiple concurrent connections
//! served by one process.

use serde_json::json;
use std::process::{Child, Command, Stdio};

struct HttpAgent {
    child: Child,
    port: u16,
    token: Option<String>,
}

impl Drop for HttpAgent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The bind → drop → spawn window must not interleave with a sibling
/// test's: another test could grab the dropped port and this agent would
/// fail to bind (its `wait_for_port` would then connect to the thief).
static SPAWN: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn spawn_http_agent(dir: &std::path::Path, name: &str) -> HttpAgent {
    spawn_http_agent_with_token(dir, name, None)
}

fn spawn_http_agent_with_token(
    dir: &std::path::Path,
    name: &str,
    token: Option<&str>,
) -> HttpAgent {
    let _spawn = SPAWN.lock().unwrap_or_else(|poison| poison.into_inner());
    std::fs::create_dir_all(dir.join(name)).unwrap();
    // Pick a free port by binding one first.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let mut command = Command::new(env!("CARGO_BIN_EXE_agentkit-tackle"));
    command.args([
        "http",
        "--bind",
        "127.0.0.1",
        "--http-port",
        &port.to_string(),
        "--config-dir",
        dir.join(name).to_str().unwrap(),
        "--db-path",
        dir.join("sessions.db").to_str().unwrap(),
    ]);
    if let Some(token) = token {
        command.arg("--http-token").arg(token);
    }
    let child = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut agent = HttpAgent {
        child,
        port,
        token: token.map(str::to_owned),
    };
    wait_for_port(&agent);
    // A stolen port surfaces as the child exiting immediately (the
    // readiness probe may have reached the thief instead).
    if agent.child.try_wait().unwrap().is_some() {
        panic!("the HTTP agent exited instead of listening on {port}");
    }
    agent
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
    let mut request = reqwest::blocking::Client::new()
        .post(url)
        .header("content-type", "application/json");
    if let Some(token) = &agent.token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = request.body(message.to_string()).send().unwrap();
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
fn a_response_only_post_answers_immediately() {
    let dir = tempfile::tempdir().unwrap();
    let agent = spawn_http_agent(dir.path(), "a");
    wait_for_port(&agent);

    let (_, connection) = rpc(&agent, None, initialize());
    let connection = connection.expect("the connection id");

    // A client answering an agent-initiated request POSTs a response
    // (id, no method). The transport must forward it and reply `{}` —
    // never wait for a response to a response. The client timeout turns
    // the hang into a fast failure.
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let response = client
        .post(format!(
            "http://127.0.0.1:{}/rpc?connection={connection}",
            agent.port
        ))
        .header("content-type", "application/json")
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": "agent-requested-7",
                "result": { "outcome": { "outcome": "selected", "optionId": "allow" } }
            })
            .to_string(),
        )
        .send()
        .expect("the response-only POST timed out: the transport awaited a reply to a reply");
    let body = response.text().unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap(),
        json!({})
    );
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

#[test]
fn http_token_is_required_when_configured() {
    let dir = tempfile::tempdir().unwrap();
    let agent = spawn_http_agent_with_token(dir.path(), "a", Some("secret"));
    wait_for_port(&agent);

    // An unauthenticated POST is refused before it can mint a connection.
    let response = reqwest::blocking::Client::new()
        .post(format!("http://127.0.0.1:{}/rpc", agent.port))
        .header("content-type", "application/json")
        .body(initialize().to_string())
        .send()
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

    // The configured bearer token opens the connection.
    let (response, connection) = rpc(&agent, None, initialize());
    assert_eq!(response["result"]["protocolVersion"], 1, "{response}");
    assert!(connection.is_some(), "an authorised initialize gets a connection id");
}

#[test]
fn non_loopback_bind_without_token_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_agentkit-tackle"))
        .args([
            "http",
            "--bind",
            "0.0.0.0",
            "--http-port",
            "0",
            "--config-dir",
            dir.path().join("a").to_str().unwrap(),
            "--db-path",
            dir.path().join("sessions.db").to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!output.status.success(), "the bind must be refused");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("http-token"),
        "the refusal names the required token flag: {stderr}"
    );
}

#[test]
fn unknown_connection_id_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let agent = spawn_http_agent(dir.path(), "a");
    wait_for_port(&agent);

    let response = reqwest::blocking::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/rpc?connection=stale",
            agent.port
        ))
        .header("content-type", "application/json")
        .body(initialize().to_string())
        .send()
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(
        response.headers().get("x-acp-connection").is_none(),
        "a stale id must not mint a new connection"
    );
}

/// A one-shot OpenAI-compatible fixture: model discovery plus a streamed
/// `ok` completion. Runs on its own thread so the test can stay blocking.
fn model_fixture() -> String {
    use std::io::{Read as _, Write as _};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut head = [0u8; 4096];
            let read = stream.read(&mut head).unwrap_or(0);
            let request = String::from_utf8_lossy(&head[..read]);
            if request.starts_with("GET ") {
                let body = r#"{"data":[{"id":"gpt-4o-mini"}]}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            } else {
                let lines = [
                    r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{"role":"assistant"}}]}"#,
                    r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{"content":"ok"}}]}"#,
                    r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
                    "data: [DONE]",
                ];
                let body = lines.join("\n\n") + "\n";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        }
    });
    format!("http://{addr}/v1")
}

#[test]
fn v2_prompt_streams_state_updates_over_http() {
    use std::io::BufRead as _;
    use std::sync::{Arc, Mutex};

    let dir = tempfile::tempdir().unwrap();
    let base_url = model_fixture();
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    std::fs::write(
        dir.path().join("a").join("config.toml"),
        format!(
            "[endpoints.mock]\nbase_url = \"{base_url}\"\nwire_format = \"openai-chat-completions\"\nauth = \"none\"\nmodels = [\"gpt-4o-mini\"]\n\n\
             [defaults]\nmodel = \"mock/gpt-4o-mini\"\n\n\
             [scripts.titling]\nevents = [\"title_trigger\"]\nfile = \"builtin:titling.rhai\"\nenabled = false\n"
        ),
    )
    .unwrap();
    let agent = spawn_http_agent(dir.path(), "a");
    wait_for_port(&agent);

    let (response, connection) = rpc(
        &agent,
        None,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": 2,
                "info": { "name": "test-client", "version": "1.0.0" },
                "clientCapabilities": {}
            }
        }),
    );
    assert_eq!(response["result"]["protocolVersion"], 2, "{response}");
    let connection = connection.expect("the v2 connection id");

    let frames: Arc<Mutex<Vec<String>>> = Arc::default();
    {
        let frames = Arc::clone(&frames);
        let url = format!(
            "http://127.0.0.1:{}/events?connection={connection}",
            agent.port
        );
        std::thread::spawn(move || {
            let response = reqwest::blocking::Client::new().get(url).send().unwrap();
            let mut reader = std::io::BufReader::new(response);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                if let Some(data) = line.strip_prefix("data: ") {
                    frames.lock().unwrap().push(data.trim().to_owned());
                }
                line.clear();
            }
        });
    }

    let (response, _) = rpc(
        &agent,
        Some(&connection),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/new",
            "params": { "cwd": "/http-v2", "mcpServers": [] }
        }),
    );
    let session_id = response["result"]["sessionId"]
        .as_str()
        .expect("v2 session id")
        .to_owned();

    let (response, _) = rpc(
        &agent,
        Some(&connection),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": "hello" }]
            }
        }),
    );
    let message_id = response["result"]["messageId"]
        .as_str()
        .expect("the insertion message id")
        .to_owned();
    assert!(!message_id.is_empty());

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut saw_running = false;
    let mut saw_idle = false;
    while std::time::Instant::now() < deadline && !(saw_running && saw_idle) {
        let frames = frames.lock().unwrap().clone();
        saw_running = frames.iter().any(|frame| {
            frame.contains("\"sessionUpdate\":\"state_update\"")
                && frame.contains("\"state\":\"running\"")
        });
        saw_idle = frames.iter().any(|frame| {
            frame.contains("\"sessionUpdate\":\"state_update\"")
                && frame.contains("\"state\":\"idle\"")
                && frame.contains("\"stopReason\":\"end_turn\"")
        });
        if saw_running && saw_idle {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let frames = frames.lock().unwrap().clone();
    assert!(saw_running, "running state over SSE: {frames:?}");
    assert!(saw_idle, "idle state over SSE: {frames:?}");
    assert!(
        frames.iter().any(|frame| frame.contains(&message_id)),
        "the user message id rides the SSE stream: {frames:?}"
    );
}
