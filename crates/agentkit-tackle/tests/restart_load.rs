//! AC-002 persistence (model-free): a turn created by direct
//! invocation (AC-006) survives process restart. `session/load` must
//! replay the full user-visible history with stable message and
//! tool-call ids and send exactly one usage snapshot (FR-005,
//! FR-007). Direct invocation creates real turn history without a
//! model, so no endpoint is ever contacted.
//!
//! Supersedes the empty-session load check in `initialize.rs`
//! (`session_load_on_an_empty_session_answers`) — that one loads a
//! session with no turns and therefore asserts nothing about replay.

use agentkit_tackle::agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, ListSessionsRequest, LoadSessionRequest, NewSessionRequest,
    PromptRequest, SessionId, SessionNotification, SessionUpdate, StopReason, TextContent,
};
use agentkit_tackle::agent_client_protocol::schema::ProtocolVersion;
use agentkit_tackle::agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo};
use std::sync::{Arc as StdArc, Mutex as StdMutex};

/// One replayed notification: (kind, id, payload).
type ReplayEntry = (String, String, String);

#[derive(Default)]
struct Seen {
    replay: Vec<ReplayEntry>,
    usage_snapshots: usize,
}

fn chunk_text(block: &agentkit_tackle::agent_client_protocol::schema::v1::ContentBlock) -> String {
    match block {
        agentkit_tackle::agent_client_protocol::schema::v1::ContentBlock::Text(text) => {
            text.text.clone()
        }
        _ => String::new(),
    }
}

fn write_config(dir: &std::path::Path) {
    let cfg = dir.join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    // No endpoint is contacted (direct invocation runs no model), but
    // the model reference makes a post-replay usage snapshot
    // well-defined: context_window("dead/MiniMax-M2") is 204_800.
    std::fs::write(
        cfg.join("config.toml"),
        format!(
            "[endpoints.dead]\n\
             base_url = \"http://127.0.0.1:1\"\n\
             wire_format = \"openai-chat-completions\"\n\
             auth = \"none\"\n\
             models = [\"MiniMax-M2\"]\n\
             \n\
             [defaults]\n\
             model = \"dead/MiniMax-M2\"\n\
             \n\
             [mcp_servers.echo]\n\
             transport = \"stdio\"\n\
             command = {}\n",
            serde_json::to_string(env!("CARGO_BIN_EXE_tackle-mcp-echo")).expect("path serialises"),
        ),
    )
    .unwrap();
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

/// Records notifications only while armed (i.e. during a `session/load`
/// window), then returns the session id, the replay, the usage-snapshot
/// count, and the last prompt's stop reason (`EndTurn` when no prompt was
/// sent). `session_id = None` creates a fresh session (and runs the
/// direct-invocation prompt when `arm_before`); `Some` loads an existing
/// session in a fresh process without touching it otherwise.
async fn load_replay(
    dir: &std::path::Path,
    arm_before: bool,
    session_id: Option<SessionId>,
) -> (SessionId, Vec<ReplayEntry>, usize, bool) {
    let agent = spawn_agent(dir);
    let seen: StdArc<StdMutex<Seen>> = StdArc::default();
    let armed = StdArc::new(StdMutex::new(false));
    let seen_handler = seen.clone();
    let armed_handler = armed.clone();

    let result = Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                if !*armed_handler.lock().unwrap() {
                    return Ok(());
                }
                let mut seen = seen_handler.lock().unwrap();
                match &notification.update {
                    SessionUpdate::UserMessageChunk(chunk) => seen.replay.push((
                        "user".to_owned(),
                        chunk
                            .message_id
                            .as_ref()
                            .map(|id| id.0.to_string())
                            .unwrap_or_default(),
                        chunk_text(&chunk.content),
                    )),
                    SessionUpdate::AgentMessageChunk(chunk) => seen.replay.push((
                        "agent".to_owned(),
                        chunk
                            .message_id
                            .as_ref()
                            .map(|id| id.0.to_string())
                            .unwrap_or_default(),
                        chunk_text(&chunk.content),
                    )),
                    SessionUpdate::ToolCall(call) => seen.replay.push((
                        "call".to_owned(),
                        call.tool_call_id.0.to_string(),
                        format!("{:?}|{}|{:?}", call.status, call.title, call.name),
                    )),
                    SessionUpdate::ToolCallUpdate(update) => seen.replay.push((
                        "update".to_owned(),
                        update.tool_call_id.0.to_string(),
                        format!("{:?}", update.fields.status),
                    )),
                    SessionUpdate::UsageUpdate(_) => seen.usage_snapshots += 1,
                    _ => {}
                }
                Ok(())
            },
            agentkit_tackle::agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let session_id = match session_id {
                Some(id) => id,
                None => {
                    let created = connection
                        .send_request(NewSessionRequest::new(std::path::PathBuf::from("/work")))
                        .block_task()
                        .await?;
                    created.session_id
                }
            };
            let mut direct_stop = true;

            if arm_before {
                // Direct invocation: real turn history, no model.
                let response = connection
                    .send_request(PromptRequest::new(
                        session_id.clone(),
                        vec![ContentBlock::Text(TextContent::new(
                            r#"/!mcp.echo.echo {"text":"persist-me"}"#.to_owned(),
                        ))],
                    ))
                    .block_task()
                    .await?;
                direct_stop = response.stop_reason == StopReason::EndTurn;
            }

            *armed.lock().unwrap() = true;
            connection
                .send_request(LoadSessionRequest::new(
                    session_id.clone(),
                    std::path::PathBuf::from("/work"),
                ))
                .block_task()
                .await?;
            // Order the notification handlers behind everything the load
            // sent (replay, snapshot, re-advertisement).
            connection
                .send_request(ListSessionsRequest::new())
                .block_task()
                .await?;
            let seen = seen.lock().unwrap();
            Ok((
                session_id,
                seen.replay.clone(),
                seen.usage_snapshots,
                direct_stop,
            ))
        })
        .await
        .expect("load scenario");
    result
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_replays_history_with_stable_ids_and_a_usage_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());

    // Phase 1: write the turn, then replay it in the same process.
    let (session_id, replay_before, usage_before, direct_ok) =
        load_replay(dir.path(), true, None).await;
    // Phase 2: fresh process on the same database, same session.
    let (_, replay_after, usage_after, _) = load_replay(dir.path(), false, Some(session_id)).await;

    let mut fails: Vec<String> = Vec::new();

    if !direct_ok {
        fails.push("direct invocation did not end with EndTurn".to_owned());
    }
    if !replay_before
        .iter()
        .any(|(kind, _, payload)| kind == "user" && payload.contains("persist-me"))
    {
        fails.push(format!(
            "replay: the user message is missing: {replay_before:?}"
        ));
    }
    if !replay_before
        .iter()
        .any(|(kind, _, payload)| kind == "user" && payload.contains("Welcome to tackle"))
    {
        fails.push(format!(
            "replay: the first-session seed is missing: {replay_before:?}"
        ));
    }
    if !replay_before.iter().any(|(kind, id, payload)| {
        kind == "call" && id.contains('-') && payload.starts_with("Completed|mcp.echo.echo")
    }) {
        fails.push(format!(
            "replay: the completed tool call is missing: {replay_before:?}"
        ));
    }

    if replay_before != replay_after {
        fails.push(format!(
            "stable ids: the replay changed across restart\n  before: {replay_before:?}\n  after:  {replay_after:?}"
        ));
    }

    if usage_before != 1 || usage_after != 1 {
        fails.push(format!(
            "usage snapshot: expected exactly one usage_update after each load, \
             got before={usage_before} after={usage_after}"
        ));
    }

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
