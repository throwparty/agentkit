//! A deterministic, offline OpenAI-compatible model endpoint used by the
//! acp-tck conformance runner. It is a test fixture, not a product binary:
//! it listens on loopback, derives every response from the request body,
//! and never reaches the network.

use axum::Json;
use axum::extract::State;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use clap::Parser;
use futures::{StreamExt, stream};
use serde::Deserialize;
use serde_json::{Value, json};
use std::convert::Infallible;
use std::net::IpAddr;
use std::process::ExitCode;
use std::time::Duration;

/// The model name echoed when a request omits one.
const DEFAULT_MODEL: &str = "tackle-test-model";
/// The completion id echoed in every response. Fixed: no clocks.
const COMPLETION_ID: &str = "tackle-test-model-completion";
/// The tool-call id used when the request requires a tool call. Fixed.
const TOOL_CALL_ID: &str = "tackle-test-model-tool-call";
/// How long a sentinel-held stream stays open before it is abandoned. The
/// runner and acp-tck apply their own, shorter, watchdogs.
const HOLD_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Parser)]
struct Args {
    /// Address to bind. Only loopback addresses are accepted.
    #[arg(long, default_value = "127.0.0.1")]
    bind: IpAddr,

    /// TCP port to listen on.
    #[arg(long)]
    port: u16,

    /// Request value that holds the response stream open until cancelled.
    #[arg(long)]
    cancel_sentinel: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatRequest {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    messages: Vec<ChatMessage>,
    #[serde(default)]
    stream: bool,
    #[serde(default)]
    tool_choice: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    role: String,
    #[serde(default)]
    content: Option<Value>,
}

impl ChatMessage {
    fn text(&self) -> String {
        text_of(self.content.as_ref())
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ToolCall {
    name: String,
    arguments: String,
}

#[derive(Debug, PartialEq, Eq)]
struct Reply {
    text: String,
    finish_reason: &'static str,
    tool_call: Option<ToolCall>,
    input_tokens: u64,
    output_tokens: u64,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    if !args.bind.is_loopback() {
        eprintln!(
            "tackle-test-model: refusing to bind non-loopback address {}",
            args.bind
        );
        return ExitCode::FAILURE;
    }

    if let Some(sentinel) = &args.cancel_sentinel {
        tracing::info!(sentinel = %sentinel, "cancel sentinel configured");
    }

    run(args.bind, args.port, args.cancel_sentinel).await
}

#[derive(Clone)]
struct AppState {
    cancel_sentinel: Option<String>,
}

async fn run(bind: IpAddr, port: u16, cancel_sentinel: Option<String>) -> ExitCode {
    let app = axum::Router::new()
        .route("/healthz", get(|| async { axum::http::StatusCode::OK }))
        .route("/v1/chat/completions", post(completions))
        .with_state(AppState { cancel_sentinel });

    let listener = match tokio::net::TcpListener::bind((bind, port)).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("tackle-test-model: cannot bind {bind}:{port}: {err}");
            return ExitCode::FAILURE;
        }
    };
    let address = listener
        .local_addr()
        .map(|address| address.to_string())
        .unwrap_or_else(|_| format!("{bind}:{port}"));
    tracing::info!(address = %address, "tackle-test-model listening");

    match axum::serve(listener, app)
        .with_graceful_shutdown(agentkit_tackle::cli::wait_for_shutdown())
        .await
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("tackle-test-model: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn completions(State(state): State<AppState>, Json(request): Json<ChatRequest>) -> Response {
    let model = request.model.as_deref().unwrap_or(DEFAULT_MODEL).to_owned();
    if request.stream && is_held(&request, state.cancel_sentinel.as_deref()) {
        return hold(model).into_response();
    }

    let reply = reply_for(&request);
    if request.stream {
        let payloads = sse_payloads(&reply, &model);
        let events: Vec<Result<Event, Infallible>> = payloads
            .into_iter()
            .map(|payload| Ok(Event::default().data(payload)))
            .chain(std::iter::once(Ok(Event::default().data("[DONE]"))))
            .collect();
        Sse::new(stream::iter(events)).into_response()
    } else {
        Json(completion_json(&reply, &model)).into_response()
    }
}

/// True when the sentinel is configured and appears in any message text.
fn is_held(request: &ChatRequest, sentinel: Option<&str>) -> bool {
    sentinel.is_some_and(|sentinel| {
        request
            .messages
            .iter()
            .any(|message| message.text().contains(sentinel))
    })
}

/// Answers with one role chunk, then holds the connection open — emitting
/// nothing further — until the client disconnects or the bounded timeout
/// elapses. This keeps a tackle turn in flight for cancellation tests.
fn hold(model: String) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let initial =
        with_model(chunk(json!({ "role": "assistant" }), Value::Null), &model).to_string();
    let events = stream::once(async move { Ok(Event::default().data(initial)) }).chain(
        stream::once(async {
            tokio::time::sleep(HOLD_TIMEOUT).await;
        })
        .filter_map(|()| std::future::ready(None)),
    );
    Sse::new(events)
}

/// Derives the entire reply — text, finish reason, and any tool call — from
/// the request body alone. No randomness, clocks, or I/O.
fn reply_for(request: &ChatRequest) -> Reply {
    let last_user = request
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(ChatMessage::text)
        .unwrap_or_default();

    let input_tokens = token_count(
        &request
            .messages
            .iter()
            .map(ChatMessage::text)
            .collect::<Vec<_>>()
            .join(" "),
    );

    if let Some(name) = required_tool_name(request.tool_choice.as_ref()) {
        let arguments = json!({ "text": last_user }).to_string();
        return Reply {
            text: String::new(),
            finish_reason: "tool_calls",
            tool_call: Some(ToolCall { name, arguments }),
            input_tokens,
            output_tokens: 0,
        };
    }

    let text = if last_user.is_empty() {
        "ok".to_owned()
    } else {
        last_user
    };
    Reply {
        output_tokens: token_count(&text),
        text,
        finish_reason: "stop",
        tool_call: None,
        input_tokens,
    }
}

/// Extracts a function name from an OpenAI `tool_choice` that names one.
fn required_tool_name(tool_choice: Option<&Value>) -> Option<String> {
    let choice = tool_choice?;
    if choice.get("type").and_then(Value::as_str) != Some("function") {
        return None;
    }
    choice
        .get("function")?
        .get("name")?
        .as_str()
        .map(str::to_owned)
}

fn text_of(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn token_count(text: &str) -> u64 {
    text.split_whitespace().count().max(1) as u64
}

fn usage_json(reply: &Reply) -> Value {
    json!({
        "prompt_tokens": reply.input_tokens,
        "completion_tokens": reply.output_tokens,
        "total_tokens": reply.input_tokens + reply.output_tokens,
    })
}

fn chunk(delta: Value, finish_reason: Value) -> Value {
    json!({
        "id": COMPLETION_ID,
        "object": "chat.completion.chunk",
        "created": 0,
        "choices": [{
            "index": 0,
            "delta": delta,
            "finish_reason": finish_reason,
        }],
    })
}

/// The SSE `data:` payloads for a streamed reply, excluding the terminal
/// `[DONE]` sentinel: a role chunk, the content or tool-call delta, and a
/// terminal chunk carrying the finish reason and usage.
fn sse_payloads(reply: &Reply, model: &str) -> Vec<String> {
    let mut payloads = Vec::with_capacity(3);
    payloads.push(with_model(
        chunk(json!({ "role": "assistant" }), Value::Null),
        model,
    ));

    let delta = match &reply.tool_call {
        Some(call) => json!({
            "tool_calls": [{
                "index": 0,
                "id": TOOL_CALL_ID,
                "type": "function",
                "function": {
                    "name": call.name,
                    "arguments": call.arguments,
                },
            }],
        }),
        None => json!({ "content": reply.text }),
    };
    payloads.push(with_model(chunk(delta, Value::Null), model));

    let mut terminal = chunk(json!({}), json!(reply.finish_reason));
    terminal["usage"] = usage_json(reply);
    payloads.push(with_model(terminal, model));

    payloads
        .into_iter()
        .map(|payload| payload.to_string())
        .collect()
}

fn completion_json(reply: &Reply, model: &str) -> Value {
    let message = match &reply.tool_call {
        Some(call) => json!({
            "role": "assistant",
            "content": Value::Null,
            "tool_calls": [{
                "id": TOOL_CALL_ID,
                "type": "function",
                "function": {
                    "name": call.name,
                    "arguments": call.arguments,
                },
            }],
        }),
        None => json!({
            "role": "assistant",
            "content": reply.text,
        }),
    };
    json!({
        "id": COMPLETION_ID,
        "object": "chat.completion",
        "created": 0,
        "model": model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": reply.finish_reason,
        }],
        "usage": usage_json(reply),
    })
}

fn with_model(mut value: Value, model: &str) -> Value {
    value["model"] = Value::String(model.to_owned());
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(body: Value) -> ChatRequest {
        serde_json::from_value(body).unwrap()
    }

    #[test]
    fn sentinel_is_detected_in_message_text() {
        let request = request(json!({
            "messages": [
                { "role": "user", "content": "please hold this turn open" }
            ]
        }));
        assert!(is_held(&request, Some("hold this turn")));
        assert!(!is_held(&request, Some("absent")));
        assert!(!is_held(&request, None));
    }

    #[test]
    fn streaming_reply_uses_the_openai_chunk_shape() {
        let reply = reply_for(&request(json!({
            "model": "test-model",
            "stream": true,
            "messages": [
                { "role": "system", "content": "be brief" },
                { "role": "user", "content": "hello there" }
            ]
        })));

        assert_eq!(reply.text, "hello there");
        assert_eq!(reply.finish_reason, "stop");

        let payloads = sse_payloads(&reply, "test-model");
        assert_eq!(payloads.len(), 3);
        for payload in &payloads {
            assert!(!payload.contains('\n'), "each payload is one SSE data line");
            let chunk: Value = serde_json::from_str(payload).unwrap();
            assert_eq!(chunk["object"], "chat.completion.chunk");
            assert_eq!(chunk["model"], "test-model");
            assert_eq!(chunk["created"], 0);
        }

        let terminal: Value = serde_json::from_str(payloads.last().unwrap()).unwrap();
        assert_eq!(terminal["choices"][0]["finish_reason"], "stop");
        assert_eq!(terminal["usage"]["prompt_tokens"], 4);
        assert_eq!(terminal["usage"]["completion_tokens"], 2);
        assert_eq!(terminal["usage"]["total_tokens"], 6);
    }

    #[test]
    fn streaming_reply_is_byte_identical_across_runs() {
        let request = request(json!({
            "model": "test-model",
            "stream": true,
            "messages": [{ "role": "user", "content": "repeat me" }]
        }));
        let first = sse_payloads(&reply_for(&request), "test-model");
        let second = sse_payloads(&reply_for(&request), "test-model");
        assert_eq!(first, second);
    }

    #[test]
    fn tool_choice_reproduces_the_tool_call_shape() {
        let reply = reply_for(&request(json!({
            "model": "test-model",
            "stream": true,
            "messages": [{ "role": "user", "content": "use a tool" }],
            "tool_choice": {
                "type": "function",
                "function": { "name": "echo" }
            }
        })));

        assert_eq!(reply.finish_reason, "tool_calls");
        let call = reply.tool_call.as_ref().unwrap();
        assert_eq!(call.name, "echo");
        assert_eq!(call.arguments, "{\"text\":\"use a tool\"}");

        let payloads = sse_payloads(&reply, "test-model");
        let delta: Value = serde_json::from_str(&payloads[1]).unwrap();
        assert_eq!(
            delta["choices"][0]["delta"]["tool_calls"][0]["type"],
            "function"
        );
        assert_eq!(
            delta["choices"][0]["delta"]["tool_calls"][0]["function"]["name"],
            "echo"
        );

        let terminal: Value = serde_json::from_str(payloads.last().unwrap()).unwrap();
        assert_eq!(terminal["choices"][0]["finish_reason"], "tool_calls");
    }

    #[test]
    fn non_streaming_reply_uses_the_completion_shape() {
        let reply = reply_for(&request(json!({
            "model": "test-model",
            "messages": [{ "role": "user", "content": "hello" }]
        })));
        let completion = completion_json(&reply, "test-model");
        assert_eq!(completion["object"], "chat.completion");
        assert_eq!(completion["choices"][0]["message"]["role"], "assistant");
        assert_eq!(completion["choices"][0]["message"]["content"], "hello");
        assert_eq!(completion["choices"][0]["finish_reason"], "stop");
        assert_eq!(completion["usage"]["total_tokens"], 2);
    }

    #[test]
    fn content_arrays_are_flattened_to_text() {
        let reply = reply_for(&request(json!({
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "one " },
                    { "type": "text", "text": "two" }
                ]
            }]
        })));
        assert_eq!(reply.text, "one two");
    }
}
