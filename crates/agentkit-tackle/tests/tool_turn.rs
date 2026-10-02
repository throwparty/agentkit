//! E2E tool-turn suite (AC-001 / AC-005 / AC-007): one scripted-model
//! session driving the seams the unit suite cannot reach (plan:
//! `adrs/2026-09-17-tackle/e2e-tests-plan.md`).
//!
//! The scenario runs over both transports — stdio (a spawned agent) and
//! HTTP (the test-side `HttpTransport` adapter against a served agent) —
//! through one shared closure: three prompts plus one reusable-prompt
//! expansion against a wiremock OpenAI-compatible endpoint whose `Respond`
//! implementation is content-driven and order-independent:
//!
//! 1. any message containing `Write a title` → plain text (the titling
//!    fork shares the endpoint and must not disturb assertions — every
//!    notification assertion filters on the session id);
//! 2. the last message containing the trigger `run the echo tool` is the
//!    current round: if the messages *after* it already carry a tool
//!    marker, the turn is done (final text `done: <marker>`); if that
//!    tail is empty, issue a tool call; otherwise answer `ok`.
//!
//! The tail rule (not "marker anywhere") is what keeps rounds 2 and 3
//! honest across the cumulative history: a marker from round 1 must not
//! end round 2 before its own tool call is issued. Rounds are told apart
//! by their argument marker — round 3 asks for `E2E-MARKER-VETO`, which
//! the `[scripts.veto]` `pre_tool_use` policy denies with a reason.
//!
//! Expected red list on first run (the four product seams plus the
//! unwired `/name` expansion) is aggregated into one failure report —
//! every assertion below records instead of aborting, so a single run
//! exposes the whole list.

use agentkit_tackle::agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, ListSessionsRequest, NewSessionRequest, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionId, SessionNotification, SessionUpdate, StopReason,
    TextContent,
};
use agentkit_tackle::agent_client_protocol::schema::ProtocolVersion;
use agentkit_tackle::agent_client_protocol::{
    AcpAgent, Agent, Channel, Client, ConnectionTo, RawJsonRpcMessage, TransportFrame,
};
use std::sync::{Arc as StdArc, Mutex as StdMutex};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// The MCP tool under test, namespaced by the registry.
const TOOL: &str = "mcp.echo.echo";
/// The tool's advertised description (manifest assertion A7).
const DESCRIPTION: &str = "Echoes the provided text";
/// The trigger phrase every round's prompt contains.
const TRIGGER: &str = "run the echo tool";
/// Round 3's trigger (classifies the veto round).
const LAST_TIME: &str = "one last time";
/// Tool-call argument the policy script vetoes.
const MARKER: &str = "E2E-MARKER-9f3a";
const VETO_MARKER: &str = "E2E-MARKER-VETO";
const VETO_REASON: &str = "vetoed by test policy";
/// The titling fork's prompt probe (its request must not disturb rules).
const TITLE_PROBE: &str = "Write a title";
/// Scripted `prompt_tokens`: A5 asserts `used == 42` per model request.
const PROMPT_TOKENS: u64 = 42;

const PROMPT_1: &str = "run the echo tool";
const PROMPT_2: &str = "run the echo tool again";
const PROMPT_3: &str = "run the echo tool one last time";
const PROMPT_GREET: &str = "/greet World";

/// Records a failure instead of aborting, so one run yields the whole
/// red list.
macro_rules! fail {
    ($fails:expr, $($arg:tt)*) => {
        ($fails.lock().unwrap()).push(format!($($arg)*))
    };
}

// ---------------------------------------------------------------------------
// Scripted model
// ---------------------------------------------------------------------------

/// Text of a wire message: string content, content-part arrays, or the
/// serialized message as a fallback (tool-call history rides here).
fn message_text(message: &serde_json::Value) -> String {
    match message.get("content") {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(""),
        _ => message.to_string(),
    }
}

fn usage_json() -> serde_json::Value {
    serde_json::json!({
        "prompt_tokens": PROMPT_TOKENS,
        "completion_tokens": 7,
        "total_tokens": PROMPT_TOKENS + 7,
    })
}

struct ScriptedModel;

impl ScriptedModel {
    /// A plain-text answer, in whichever shape the request asked for.
    fn answer(body: &serde_json::Value, text: &str) -> ResponseTemplate {
        if body["stream"].as_bool().unwrap_or(false) {
            let lines = [
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{"role":"assistant"}}]}"#.to_owned(),
                format!(
                    r#"data: {{"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{{"index":0,"delta":{{"content":{}}}}}]}}"#,
                    serde_json::to_string(text).expect("text serialises")
                ),
                format!(
                    r#"data: {{"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{{"index":0,"delta":{{}},"finish_reason":"stop"}}],"usage":{}}}"#,
                    usage_json()
                ),
                "data: [DONE]".to_owned(),
            ];
            ResponseTemplate::new(200).set_body_raw(lines.join("\n\n") + "\n", "text/event-stream")
        } else {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "c1",
                "object": "chat.completion",
                "created": 0,
                "model": "m",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": text},
                    "finish_reason": "stop",
                }],
                "usage": usage_json(),
            }))
        }
    }

    /// A tool call for `mcp.echo.echo` echoing `arg`. Rig aggregates the
    /// delta into a complete call only at `finish_reason: tool_calls`
    /// (`EMITS_COMPLETE_SINGLE_CHUNK_TOOL_CALLS` is false), so the call
    /// spans a delta chunk plus the finish chunk.
    fn tool_call(body: &serde_json::Value, arg: &str) -> ResponseTemplate {
        let arguments = serde_json::to_string(&serde_json::json!({ "text": arg }))
            .expect("arguments serialise");
        if body["stream"].as_bool().unwrap_or(false) {
            let lines = [
                r#"data: {"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{"index":0,"delta":{"role":"assistant"}}]}"#.to_owned(),
                format!(
                    r#"data: {{"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"call_e2e","type":"function","function":{{"name":"{TOOL}","arguments":{arguments}}}}}]}}}}]}}"#
                ),
                format!(
                    r#"data: {{"id":"c1","object":"chat.completion.chunk","created":0,"model":"m","choices":[{{"index":0,"delta":{{}},"finish_reason":"tool_calls"}}],"usage":{}}}"#,
                    usage_json()
                ),
                "data: [DONE]".to_owned(),
            ];
            ResponseTemplate::new(200).set_body_raw(lines.join("\n\n") + "\n", "text/event-stream")
        } else {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "c1",
                "object": "chat.completion",
                "created": 0,
                "model": "m",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": serde_json::Value::Null,
                        "tool_calls": [{
                            "id": "call_e2e",
                            "type": "function",
                            "function": {"name": TOOL, "arguments": arguments},
                        }],
                    },
                    "finish_reason": "tool_calls",
                }],
                "usage": usage_json(),
            }))
        }
    }
}

impl Respond for ScriptedModel {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let messages = body
            .get("messages")
            .and_then(|messages| messages.as_array())
            .cloned()
            .unwrap_or_default();
        let raw: Vec<String> = messages.iter().map(|m| m.to_string()).collect();

        // Rule 1: the titling fork's prompt.
        if raw.iter().any(|m| m.contains(TITLE_PROBE)) {
            return Self::answer(&body, "E2E Title");
        }

        // Rule 2/3: the current round starts at the last trigger-bearing
        // message; everything after it is this round's tail.
        if let Some(index) = raw.iter().rposition(|m| m.contains(TRIGGER)) {
            let tail = &raw[index + 1..];
            if tail.iter().any(|m| m.contains(MARKER)) {
                return Self::answer(&body, &format!("done: {MARKER}"));
            }
            if tail.iter().any(|m| m.contains(VETO_MARKER)) {
                return Self::answer(&body, &format!("done: {VETO_MARKER}"));
            }
            if tail.is_empty() {
                let arg = if raw[index].contains(LAST_TIME) {
                    VETO_MARKER
                } else {
                    MARKER
                };
                return Self::tool_call(&body, arg);
            }
            return Self::answer(&body, "ok");
        }

        // Rule 4: anything else (including `/greet World` before the
        // expansion seam is wired — A10 asserts on the request body).
        Self::answer(&body, "ok")
    }
}

// ---------------------------------------------------------------------------
// Recording client state
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct PermissionRecord {
    session: String,
    labels: Vec<String>,
    name: Option<String>,
    title: String,
}

#[derive(Debug, Default)]
struct Recorded {
    /// (session, name, title) for `session/update` tool-call announcements.
    calls: Vec<(String, Option<String>, String)>,
    /// (session, status, content-json) for tool-call updates.
    updates: Vec<(String, String, String)>,
    permissions: Vec<PermissionRecord>,
    /// (session, text) for agent message chunks.
    chunks: Vec<(String, String)>,
    /// (session, used, size) for usage updates.
    usage: Vec<(String, u64, u64)>,
}

fn session_key(id: &SessionId) -> String {
    id.0.to_string()
}

fn block_text(block: &agentkit_tackle::agent_client_protocol::schema::v1::ContentBlock) -> String {
    match block {
        agentkit_tackle::agent_client_protocol::schema::v1::ContentBlock::Text(text) => {
            text.text.clone()
        }
        _ => String::new(),
    }
}

/// Sends one prompt and orders the notification handlers behind its
/// notifications; failures record instead of aborting the scenario.
async fn prompt_turn(
    connection: &ConnectionTo<Agent>,
    session_id: &SessionId,
    text: &str,
    label: &str,
    fails: &StdArc<StdMutex<Vec<String>>>,
) {
    match connection
        .send_request(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new(text.to_owned()))],
        ))
        .block_task()
        .await
    {
        Ok(response) => {
            if response.stop_reason != StopReason::EndTurn {
                fail!(
                    fails,
                    "{label}: stop reason {:?}, expected EndTurn",
                    response.stop_reason
                );
            }
        }
        Err(err) => fail!(fails, "{label}: session/prompt failed: {err}"),
    }
    if let Err(err) = connection
        .send_request(ListSessionsRequest::new())
        .block_task()
        .await
    {
        fail!(fails, "{label}: notification flush failed: {err}");
    }
}

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

const VETO_SCRIPT: &str = r#"fn pre_tool_use(request) {
    if request.arguments.text == "E2E-MARKER-VETO" {
        #{ decision: "deny", reason: "vetoed by test policy" }
    } else {
        #{ decision: "ask" }
    }
}
"#;

const GREET_PROMPT: &str = r#"+++
description = "Greet someone"
parameters = ["who"]
+++
Hello, {{ who }}!
"#;

fn write_config(dir: &std::path::Path, endpoint_uri: &str) {
    let cfg = dir.join("cfg");
    std::fs::create_dir_all(cfg.join("scripts")).unwrap();
    std::fs::create_dir_all(cfg.join("prompts")).unwrap();
    std::fs::write(
        cfg.join("config.toml"),
        format!(
            r#"[endpoints.mock]
base_url = "{endpoint_uri}"
wire_format = "openai-chat-completions"
auth = "none"
models = ["MiniMax-M2"]

[defaults]
model = "mock/MiniMax-M2"

[mcp_servers.echo]
transport = "stdio"
command = {echo}

[scripts.veto]
events = ["pre_tool_use"]
file = "veto.rhai"
"#,
            endpoint_uri = endpoint_uri,
            echo = serde_json::to_string(env!("CARGO_BIN_EXE_tackle-mcp-echo"))
                .expect("path serialises"),
        ),
    )
    .unwrap();
    std::fs::write(cfg.join("scripts").join("veto.rhai"), VETO_SCRIPT).unwrap();
    std::fs::write(cfg.join("prompts").join("greet.md"), GREET_PROMPT).unwrap();
}

fn spawn_agent(dir: &std::path::Path) -> AcpAgent {
    AcpAgent::from_args([
        env!("CARGO_BIN_EXE_agentkit-tackle"),
        "stdio",
        "--config-dir",
        dir.join("cfg").to_str().unwrap(),
        "--db-path",
        dir.join("sessions.db").to_str().unwrap(),
    ])
    .expect("agent arguments")
}

// ---------------------------------------------------------------------------
// The scenario (transport-generic: the same closure drives stdio and HTTP)
// ---------------------------------------------------------------------------

/// Runs the scripted session over any transport: client handlers, the four
/// prompts, and the in-scenario assertions (A1–A5, A8, A9-call-shape).
async fn scenario_over(
    transport: impl agentkit_tackle::agent_client_protocol::ConnectTo<Client>,
    seen: StdArc<StdMutex<Recorded>>,
    fails: StdArc<StdMutex<Vec<String>>>,
) -> Result<(), agentkit_tackle::agent_client_protocol::Error> {
    let seen_notifications = seen.clone();
    let seen_permissions = seen.clone();
    let fails_permissions = fails.clone();
    let fails_scenario = fails.clone();

    Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                let session = session_key(&notification.session_id);
                let mut seen = seen_notifications.lock().unwrap();
                match &notification.update {
                    SessionUpdate::ToolCall(call) => {
                        seen.calls
                            .push((session, call.name.clone(), call.title.clone()))
                    }
                    SessionUpdate::ToolCallUpdate(update) => seen.updates.push((
                        session,
                        update
                            .fields
                            .status
                            .map(|status| format!("{status:?}"))
                            .unwrap_or_default(),
                        update
                            .fields
                            .content
                            .as_ref()
                            .map(|content| serde_json::to_string(content).unwrap_or_default())
                            .unwrap_or_default(),
                    )),
                    SessionUpdate::AgentMessageChunk(chunk) => {
                        seen.chunks.push((session, block_text(&chunk.content)));
                    }
                    SessionUpdate::UsageUpdate(usage) => {
                        seen.usage.push((session, usage.used, usage.size));
                    }
                    _ => {}
                }
                Ok(())
            },
            agentkit_tackle::agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| {
                let labels: Vec<String> = request
                    .options
                    .iter()
                    .map(|option| option.name.clone())
                    .collect();
                let first_ask = {
                    let mut seen = seen_permissions.lock().unwrap();
                    let first = seen.permissions.is_empty();
                    seen.permissions.push(PermissionRecord {
                        session: session_key(&request.session_id),
                        labels: labels.clone(),
                        name: request.tool_call.fields.name.clone(),
                        title: request.tool_call.fields.title.clone().unwrap_or_default(),
                    });
                    first
                };
                // Round 1 grants the session-scoped allow (A8 asserts no
                // second ask); anything later answers allow-once.
                let wanted = if first_ask {
                    "Allow for this session"
                } else {
                    "Allow once"
                };
                match request
                    .options
                    .iter()
                    .find(|option| option.name == wanted)
                    .or_else(|| request.options.first())
                {
                    Some(option) => responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                            option.option_id.clone(),
                        )),
                    )),
                    None => {
                        fail!(
                            fails_permissions,
                            "A2: permission request carried no options: {labels:?}"
                        );
                        responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Cancelled,
                        ))
                    }
                }
            },
            agentkit_tackle::agent_client_protocol::on_receive_request!(),
        )
        .connect_with(transport, |connection: ConnectionTo<Agent>| async move {
            let fails = fails_scenario;
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let created = connection
                .send_request(NewSessionRequest::new(std::path::PathBuf::from("/work")))
                .block_task()
                .await?;
            let sid = created.session_id.clone();
            let key = session_key(&sid);
            connection
                .send_request(ListSessionsRequest::new())
                .block_task()
                .await?;

            // Usage cadence baseline: A5 counts this turn's requests.
            let usage_before = seen
                .lock()
                .unwrap()
                .usage
                .iter()
                .filter(|(session, used, _)| session == &key && *used == PROMPT_TOKENS)
                .count();

            // --- Round 1: tool call, permission ask, allow-for-session ---
            prompt_turn(&connection, &sid, PROMPT_1, "prompt 1 (tool round)", &fails).await;
            {
                let seen = seen.lock().unwrap();
                let calls: Vec<&(String, Option<String>, String)> =
                    seen.calls.iter().filter(|(s, _, _)| s == &key).collect();
                if !calls
                    .iter()
                    .any(|(_, name, title)| name.as_deref() == Some(TOOL) || title.contains(TOOL))
                {
                    fail!(fails, "A1: no ToolCall announcement for {TOOL}: {calls:?}");
                }

                let perms: Vec<&PermissionRecord> = seen
                    .permissions
                    .iter()
                    .filter(|p| p.session == key)
                    .collect();
                if perms.len() != 1 {
                    fail!(
                        fails,
                        "A2: expected 1 permission request after prompt 1, got {}: {:?}",
                        perms.len(),
                        perms.iter().map(|p| &p.labels).collect::<Vec<_>>()
                    );
                }
                if let Some(perm) = perms.first() {
                    if !(perm.labels.iter().any(|l| l == "Allow once")
                        && perm.labels.iter().any(|l| l == "Allow for this session"))
                    {
                        fail!(
                            fails,
                            "A2: labels missing the honest scope pair: {:?}",
                            perm.labels
                        );
                    }
                    if !(perm.name.as_deref() == Some(TOOL) || perm.title.contains(TOOL)) {
                        fail!(
                            fails,
                            "A2: permission tool_call does not name {TOOL}: name={:?} title={:?}",
                            perm.name,
                            perm.title
                        );
                    }
                }

                let completed = seen
                    .updates
                    .iter()
                    .filter(|(s, status, content)| {
                        s == &key && status == "Completed" && content.contains(MARKER)
                    })
                    .count();
                if completed == 0 {
                    fail!(
                        fails,
                        "A3: no Completed tool update carrying {MARKER}: {:?}",
                        seen.updates
                            .iter()
                            .filter(|(s, _, _)| s == &key)
                            .map(|(_, status, content)| (status.clone(), content.clone()))
                            .collect::<Vec<_>>()
                    );
                }

                let text: String = seen
                    .chunks
                    .iter()
                    .filter(|(s, _)| s == &key)
                    .map(|(_, text)| text.as_str())
                    .collect();
                if !text.contains(&format!("done: {MARKER}")) {
                    fail!(fails, "A4: final text missing done: {MARKER}: {text:?}");
                }

                let usage_now = seen
                    .usage
                    .iter()
                    .filter(|(session, used, _)| session == &key && *used == PROMPT_TOKENS)
                    .count();
                if usage_now - usage_before < 2 {
                    fail!(
                        fails,
                        "A5: {usage} usage_update(s) with used=={PROMPT_TOKENS} after \
                         prompt 1 (baseline {usage_before}), expected >= 2 — one per \
                         model request in the turn",
                        usage = usage_now - usage_before
                    );
                }
            }

            // --- Round 2: allow-always grant, no second ask ---
            prompt_turn(
                &connection,
                &sid,
                PROMPT_2,
                "prompt 2 (grant round)",
                &fails,
            )
            .await;
            {
                let seen = seen.lock().unwrap();
                let asks = seen.permissions.iter().filter(|p| p.session == key).count();
                if asks != 1 {
                    fail!(
                        fails,
                        "A8: permission request count after prompt 2 is {asks}, \
                         expected 1 (prompt 1's single ask, unchanged — allow-always \
                         must be honoured in-session)"
                    );
                }
            }

            // --- Round 3: the veto round ---
            prompt_turn(&connection, &sid, PROMPT_3, "prompt 3 (veto round)", &fails).await;
            {
                let seen = seen.lock().unwrap();
                if let Some((_, status, content)) = seen
                    .updates
                    .iter()
                    .filter(|(s, _, _)| s == &key)
                    .find(|(_, _, content)| content.contains(VETO_MARKER))
                {
                    if status == "Completed" {
                        fail!(
                            fails,
                            "A9: the vetoed call reported Completed with output: {content}"
                        );
                    }
                }
            }

            // --- AC-007: /name expansion into the conversation ---
            prompt_turn(&connection, &sid, PROMPT_GREET, "prompt 4 (/greet)", &fails).await;

            Ok(())
        })
        .await
}

/// A6, A7, A9-reason, A10: assertions on what the mock *received*.
async fn assert_mock_received(server: &MockServer, fails: &StdArc<StdMutex<Vec<String>>>) {
    let requests = server.received_requests().await.unwrap_or_default();
    let bodies: Vec<String> = requests
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect();
    let parsed: Vec<serde_json::Value> = bodies
        .iter()
        .filter_map(|body| serde_json::from_str(body).ok())
        .collect();

    let tools_offer = parsed.iter().any(|body| {
        body["tools"].as_array().is_some_and(|tools| {
            tools.iter().any(|tool| {
                tool["function"]["name"].as_str() == Some(TOOL)
                    || tool["name"].as_str() == Some(TOOL)
            })
        })
    });
    if !tools_offer {
        fail!(
            fails,
            "A6: no model request carried a tools array containing {TOOL} \
             ({} requests seen)",
            parsed.len()
        );
    }

    let manifest = parsed.iter().any(|body| {
        let system: String = body["messages"]
            .as_array()
            .map(|messages| {
                messages
                    .iter()
                    .filter(|message| message["role"] == "system")
                    .map(message_text)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        system.contains(&format!("!{TOOL}")) && system.contains(DESCRIPTION)
    });
    if !manifest {
        fail!(
            fails,
            "A7: no system message carried the invokable manifest \
             (!{TOOL} plus its description)"
        );
    }

    if !bodies.iter().any(|body| body.contains(VETO_REASON)) {
        fail!(
            fails,
            "A9: the deny reason `{VETO_REASON}` never reached a model request"
        );
    }

    let greet_literal = bodies.iter().any(|body| body.contains(PROMPT_GREET));
    let greet_expanded = bodies.iter().any(|body| body.contains("Hello, World!"));
    if greet_literal || !greet_expanded {
        fail!(
            fails,
            "A10: /greet World reached the model unexpanded (literal seen: \
             {greet_literal}, expansion seen: {greet_expanded})"
        );
    }
}

/// One report: every recorded failure, never just the first.
fn report_red_list(fails: &StdArc<StdMutex<Vec<String>>>) {
    let fails = fails.lock().unwrap();
    assert!(
        fails.is_empty(),
        "\n{} red assertion(s):\n{}\n",
        fails.len(),
        fails
            .iter()
            .enumerate()
            .map(|(index, failure)| format!("{}. {failure}", index + 1))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn scripted_model_tool_turn_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    write_config(dir.path(), &server.uri());

    let fails: StdArc<StdMutex<Vec<String>>> = StdArc::default();
    scenario_over(spawn_agent(dir.path()), StdArc::default(), fails.clone())
        .await
        .expect("stdio scenario");
    assert_mock_received(&server, &fails).await;
    report_red_list(&fails);
}

// ---------------------------------------------------------------------------
// The HTTP run: a test-side transport adapter (plan §5.3)
// ---------------------------------------------------------------------------

/// Frames → `POST /rpc` bodies; POST bodies + one `GET /events` SSE stream
/// → incoming frames. The stock `HttpClient` (plan §5.3's mismatch table)
/// would hang on every request; this is the shape our `http.rs` serves.
struct HttpTransport {
    base_url: String,
}

impl agentkit_tackle::agent_client_protocol::ConnectTo<Client> for HttpTransport {
    async fn connect_to(
        self,
        client: impl agentkit_tackle::agent_client_protocol::ConnectTo<Agent>,
    ) -> Result<(), agentkit_tackle::agent_client_protocol::Error> {
        let (http_end, client_end) = Channel::duplex();
        let client_task = client.connect_to(client_end);
        let bridge_task = bridge_frames(http_end, self.base_url);
        match futures::future::select(Box::pin(client_task), Box::pin(bridge_task)).await {
            futures::future::Either::Left((result, _bridge)) => result,
            futures::future::Either::Right((result, _client)) => result,
        }
    }
}

fn bridge_error(message: String) -> agentkit_tackle::agent_client_protocol::Error {
    agentkit_tackle::agent_client_protocol::Error::internal_error().data(message)
}

/// One POST body worth feeding back to the client loop: a real JSON-RPC
/// response, never the transport's `{}` acknowledgement.
fn incoming_frame(body: &str) -> Option<TransportFrame> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let object = value.as_object()?;
    if object.contains_key("method")
        || !(object.contains_key("result") || object.contains_key("error"))
    {
        return None;
    }
    match TransportFrame::parse_json(body) {
        TransportFrame::Malformed { .. } => None,
        frame => Some(frame),
    }
}

async fn post_line(
    http: &reqwest::Client,
    base_url: &str,
    connection: &StdArc<StdMutex<Option<String>>>,
    body: String,
) -> Result<String, agentkit_tackle::agent_client_protocol::Error> {
    let url = match connection.lock().unwrap().clone() {
        Some(id) => format!("{base_url}/rpc?connection={id}"),
        None => format!("{base_url}/rpc"),
    };
    let response = http
        .post(&url)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|err| bridge_error(format!("POST {url}: {err}")))?;
    if connection.lock().unwrap().is_none() {
        if let Some(id) = response
            .headers()
            .get("x-acp-connection")
            .and_then(|value| value.to_str().ok())
        {
            *connection.lock().unwrap() = Some(id.to_owned());
        }
    }
    if !response.status().is_success() {
        return Err(bridge_error(format!("POST {url}: {}", response.status())));
    }
    response
        .text()
        .await
        .map_err(|err| bridge_error(format!("POST {url}: {err}")))
}

/// Follows `GET /events?connection=` until the server closes the stream:
/// every `data:` line becomes one incoming frame. `ready` fires as soon as
/// the response headers land — the server subscribes to the broadcast
/// before answering, so by then no notification can slip past.
async fn run_sse(
    http: reqwest::Client,
    url: String,
    tx: futures::channel::mpsc::UnboundedSender<TransportFrame>,
    ready: tokio::sync::oneshot::Sender<()>,
) {
    let mut ready = Some(ready);
    let mut response = match http.get(&url).send().await {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(%err, %url, "event stream failed to start");
            if let Some(ready) = ready.take() {
                let _ = ready.send(());
            }
            return;
        }
    };
    if let Some(ready) = ready.take() {
        let _ = ready.send(());
    }
    let mut buffer = String::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(newline) = buffer.find('\n') {
            let line: String = buffer.drain(..=newline).collect();
            let line = line.trim_end_matches(['\r', '\n']);
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.strip_prefix(' ').unwrap_or(data);
            match TransportFrame::parse_json(data) {
                TransportFrame::Malformed { .. } => {}
                frame => {
                    let _ = tx.unbounded_send(frame);
                }
            }
        }
    }
}

async fn bridge_frames(
    endpoint: Channel,
    base_url: String,
) -> Result<(), agentkit_tackle::agent_client_protocol::Error> {
    use futures::StreamExt as _;

    let http = reqwest::Client::new();
    let connection: StdArc<StdMutex<Option<String>>> = StdArc::default();
    let Channel { mut rx, tx } = endpoint;
    let mut sse_started = false;

    while let Some(frame) = rx.next().await {
        let response_only = matches!(
            &frame,
            TransportFrame::Single(RawJsonRpcMessage::Response(_))
        );
        let body = frame
            .to_json()
            .map_err(|err| bridge_error(err.to_string()))?;
        if response_only {
            // Answering an agent-initiated request must not queue behind a
            // long-running request POST (plan §4.4's deadlock): concurrent.
            let http = http.clone();
            let connection = connection.clone();
            let base_url = base_url.clone();
            tokio::spawn(async move {
                let _ = post_line(&http, &base_url, &connection, body).await;
            });
            continue;
        }
        // Requests and notifications keep their order: one awaited lane.
        let text = post_line(&http, &base_url, &connection, body).await?;
        if !sse_started {
            let established = connection.lock().unwrap().clone();
            if let Some(id) = established {
                sse_started = true;
                let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
                tokio::spawn(run_sse(
                    http.clone(),
                    format!("{base_url}/events?connection={id}"),
                    tx.clone(),
                    ready_tx,
                ));
                // No second frame leaves until the stream is subscribed.
                let _ = ready_rx.await;
            }
        }
        if let Some(frame) = incoming_frame(&text) {
            let _ = tx.unbounded_send(frame);
        }
    }
    Ok(())
}

struct HttpAgentChild {
    child: std::process::Child,
    port: u16,
}

impl Drop for HttpAgentChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_http_agent(dir: &std::path::Path) -> HttpAgentChild {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let child = std::process::Command::new(env!("CARGO_BIN_EXE_agentkit-tackle"))
        .args([
            "http",
            "--bind",
            "127.0.0.1",
            "--http-port",
            &port.to_string(),
            "--config-dir",
            dir.join("cfg").to_str().unwrap(),
            "--db-path",
            dir.join("sessions.db").to_str().unwrap(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    let mut agent = HttpAgentChild { child, port };
    let mut ready = false;
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            ready = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !ready {
        let _ = agent.child.kill();
        panic!("the HTTP agent never listened");
    }
    if agent.child.try_wait().unwrap().is_some() {
        panic!("the HTTP agent exited instead of listening on {port}");
    }
    agent
}

#[tokio::test(flavor = "multi_thread")]
async fn scripted_model_tool_turn_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ScriptedModel)
        .mount(&server)
        .await;
    write_config(dir.path(), &server.uri());
    let agent = spawn_http_agent(dir.path());

    let fails: StdArc<StdMutex<Vec<String>>> = StdArc::default();
    let transport = HttpTransport {
        base_url: format!("http://127.0.0.1:{}", agent.port),
    };
    scenario_over(transport, StdArc::default(), fails.clone())
        .await
        .expect("http scenario");
    assert_mock_received(&server, &fails).await;
    report_red_list(&fails);
}

// ---------------------------------------------------------------------------
// Live-model smoke (manual)
// ---------------------------------------------------------------------------

/// One real prompt: a tool call through the echo server, then `end_turn`.
/// Gated hard — `#[ignore]`, spend-money flag, and endpoint env vars —
/// stdio only. See plan §7.3.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "spends real money; run manually with AGENTKIT_TESTS_CAN_SPEND_MONEY=1"]
async fn live_model_tool_turn() {
    let skip = |reason: &str| {
        eprintln!("live_model_tool_turn skipped: {reason}");
    };
    if std::env::var("AGENTKIT_TESTS_CAN_SPEND_MONEY").as_deref() != Ok("1") {
        return skip("AGENTKIT_TESTS_CAN_SPEND_MONEY != 1");
    }
    let (Ok(base_url), Ok(model), Ok(_token)) = (
        std::env::var("AGENTKIT_E2E_BASE_URL"),
        std::env::var("AGENTKIT_E2E_MODEL"),
        std::env::var("AGENTKIT_E2E_TOKEN"),
    ) else {
        return skip("AGENTKIT_E2E_BASE_URL / _MODEL / _TOKEN unset");
    };

    let dir = tempfile::tempdir().unwrap();

    // The credential rides the helper protocol: a PATH-resolved
    // `agentkit-credential-live` printing the token from the environment
    // the spawned agent inherits.
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let helper = bin.join("agentkit-credential-live");
    std::fs::write(
        &helper,
        "#!/bin/sh\n\
         [ \"$#\" -eq 3 ] && [ \"$1\" = get ] && [ \"$2\" = tackle ] || exit 3\n\
         printf '{\"access_token\": \"%s\"}' \"$AGENTKIT_E2E_TOKEN\"\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    std::env::set_var("PATH", path);

    let cfg = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("config.toml"),
        format!(
            "credential_helper = \"live\"\n\
             \n\
             [endpoints.live]\n\
             base_url = \"{base_url}\"\n\
             wire_format = \"openai-chat-completions\"\n\
             auth = \"helper\"\n\
             models = [{model}]\n\
             \n\
             [defaults]\n\
             model = \"live/{model}\"\n\
             \n\
             [mcp_servers.echo]\n\
             transport = \"stdio\"\n\
             command = {echo}\n",
            base_url = base_url,
            model = serde_json::to_string(&model).expect("model serialises"),
            echo = serde_json::to_string(env!("CARGO_BIN_EXE_tackle-mcp-echo"))
                .expect("path serialises"),
        ),
    )
    .unwrap();

    let agent = spawn_agent(dir.path());
    let calls: StdArc<StdMutex<Vec<String>>> = StdArc::default();
    let text: StdArc<StdMutex<String>> = StdArc::default();
    let calls_handler = calls.clone();
    let text_handler = text.clone();

    Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                match &notification.update {
                    SessionUpdate::ToolCall(call) => {
                        calls_handler.lock().unwrap().push(
                            call.name
                                .clone()
                                .unwrap_or_else(|| call.title.clone()),
                        );
                    }
                    SessionUpdate::AgentMessageChunk(chunk) => {
                        if let agentkit_tackle::agent_client_protocol::schema::v1::
                            ContentBlock::Text(body) =
                            &chunk.content
                        {
                            text_handler.lock().unwrap().push_str(&body.text);
                        }
                    }
                    _ => {}
                }
                Ok(())
            },
            agentkit_tackle::agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| match request
                .options
                .iter()
                .find(|option| option.name == "Allow for this session")
                .or_else(|| request.options.first())
            {
                Some(option) => responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                        option.option_id.clone(),
                    )),
                )),
                None => responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Cancelled,
                )),
            },
            agentkit_tackle::agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let created = connection
                .send_request(NewSessionRequest::new(std::path::PathBuf::from("/work")))
                .block_task()
                .await?;
            let response = connection
                .send_request(PromptRequest::new(
                    created.session_id,
                    vec![ContentBlock::Text(TextContent::new(
                        "Use the mcp.echo.echo tool to echo the text \"ping\", then \
                         reply with the single word: done"
                            .to_owned(),
                    ))],
                ))
                .block_task()
                .await?;
            assert_eq!(response.stop_reason, StopReason::EndTurn);
            Ok(())
        })
        .await
        .expect("live scenario");

    let calls = calls.lock().unwrap();
    assert!(
        calls.iter().any(|name| name == TOOL),
        "the live model called {TOOL}: {calls:?}"
    );
    let text = text.lock().unwrap();
    assert!(!text.is_empty(), "the live model produced a final answer");
}
