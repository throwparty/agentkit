//! Offline tests for the `tackle-test-model` conformance fixture: the
//! streaming and non-streaming reply shapes, the held-open sentinel, and
//! client cancellation. Each test starts the fixture on an ephemeral loopback
//! port; no test reaches the network.

use serde_json::{Value, json};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const SENTINEL: &str = "TACKLE-SENTINEL-HOLD";

struct TestModel {
    child: Child,
    base_url: String,
}

impl TestModel {
    fn spawn(sentinel: Option<&str>) -> Self {
        for _ in 0..10 {
            let port = free_port();
            let mut command = Command::new(env!("CARGO_BIN_EXE_tackle-test-model"));
            command
                .arg("--port")
                .arg(port.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if let Some(sentinel) = sentinel {
                command.arg("--cancel-sentinel").arg(sentinel);
            }
            let child = command.spawn().expect("spawn tackle-test-model");
            if wait_until_listening(port) {
                return Self {
                    child,
                    base_url: format!("http://127.0.0.1:{port}"),
                };
            }
            let mut child = child;
            let _ = child.kill();
            let _ = child.wait();
        }
        panic!("tackle-test-model never started listening");
    }

    fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("build reqwest client")
    }

    fn completions_url(&self) -> String {
        format!("{}/v1/chat/completions", self.base_url)
    }
}

impl Drop for TestModel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("read ephemeral address")
        .port()
}

fn wait_until_listening(port: u16) -> bool {
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn chat_request(content: &str, stream: bool) -> Value {
    json!({
        "model": "test-model",
        "stream": stream,
        "messages": [{ "role": "user", "content": content }],
    })
}

#[tokio::test]
async fn test_model_streaming_reply_shape() {
    let server = TestModel::spawn(None);
    let response = server
        .client()
        .post(server.completions_url())
        .json(&chat_request("hello there", true))
        .send()
        .await
        .expect("streaming request");

    assert_eq!(response.status(), 200);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(
        content_type.starts_with("text/event-stream"),
        "content type: {content_type}"
    );

    let body = response.text().await.expect("read stream body");
    assert!(
        body.contains("\"delta\":{\"role\":\"assistant\"}"),
        "{body}"
    );
    assert!(body.contains("\"content\":\"hello there\""), "{body}");
    assert!(body.contains("\"finish_reason\":\"stop\""), "{body}");
    assert!(body.contains("\"usage\":{"), "{body}");
    assert!(body.trim_end().ends_with("data: [DONE]"), "{body}");
}

#[tokio::test]
async fn test_model_non_streaming_reply_shape() {
    let server = TestModel::spawn(None);
    let response = server
        .client()
        .post(server.completions_url())
        .json(&chat_request("hello there", false))
        .send()
        .await
        .expect("non-streaming request");

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("parse completion");
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["message"]["role"], "assistant");
    assert_eq!(body["choices"][0]["message"]["content"], "hello there");
    assert_eq!(body["choices"][0]["finish_reason"], "stop");
    assert!(body["usage"]["total_tokens"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn test_model_held_sentinel_stays_open_until_cancelled() {
    let server = TestModel::spawn(Some(SENTINEL));
    let client = server.client();

    let mut held = client
        .post(server.completions_url())
        .json(&chat_request(&format!("please {SENTINEL} now"), true))
        .send()
        .await
        .expect("held request");
    assert_eq!(held.status(), 200);

    let first = held.chunk().await.expect("first chunk").expect("chunk");
    let first = String::from_utf8_lossy(&first);
    assert!(first.contains("\"role\":\"assistant\""), "{first}");
    assert!(!first.contains("\"finish_reason\":\"stop\""), "{first}");

    let next = tokio::time::timeout(Duration::from_millis(400), held.chunk()).await;
    assert!(
        next.is_err(),
        "the held stream must not complete on its own"
    );

    // Cancelling the client drops the response and closes the connection.
    drop(held);

    // The endpoint is still serving: the held stream did not wedge it.
    let normal = tokio::time::timeout(
        Duration::from_secs(5),
        client
            .post(server.completions_url())
            .json(&chat_request("after cancel", false))
            .send(),
    )
    .await
    .expect("endpoint is not wedged")
    .expect("normal request after cancellation");
    assert_eq!(normal.status(), 200);
    let body: Value = normal.json().await.expect("parse completion");
    assert_eq!(body["choices"][0]["message"]["content"], "after cancel");
}
