//! The `session/prompt` dispatch cases: the slash-command intercepts (`/fork`,
//! `/!`, `/mcp`, `/editor`, compaction) and the model turn, each lifted out of
//! the request handler so it reads as a short ladder.

use super::capabilities::UnstableFeature;
use super::commands::{available_commands_update, editor_command, mcp_command};
use super::negotiation::ConnectionNegotiation;
use super::notices::connection_notices;
use super::sink::usage_update;
use super::support::{
    append_assistant_text, send_agent_message_chunk, send_updated_at, turn_runtime, Cx,
};
use super::wire_kind;
use crate::acp::common::dispatch::{decide, Dispatch};
use crate::agent::compaction;
use crate::agent::config_options::{effective_context_window, resolve_model_for, session_actor};
use crate::agent::direct::run_direct_invocation;
use crate::agent::fork::{self, fork_access};
use crate::agent::prompt::run_prompt_turn;
use crate::state::{store_session_id, wire_session_id, TackleState};
use agent_client_protocol::schema::v1::{
    CompactionId, CompactionStatus, CompactionUpdate, ContentBlock, PromptRequest, PromptResponse,
    SessionId, SessionNotification, SessionUpdate, StopReason, TextContent, ToolCall, ToolCallId,
    ToolCallStatus,
};
use agent_client_protocol::{Error, Responder};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(super) use crate::acp::common::session::{BeginError, BegunTurn};

/// Acquires the one-turn-at-a-time lease and persists the prompt turn and its
/// user message from a v1 prompt request. The returned heartbeat must be held
/// for the turn's lifetime; dropping it stops renewal.
pub(super) async fn begin_prompt_turn(
    state: &TackleState,
    request: &PromptRequest,
    owner: &str,
) -> Result<BegunTurn, BeginError> {
    let store_session = store_session_id(&request.session_id);
    let blocks = request
        .prompt
        .iter()
        .map(|block| {
            crate::acp::common::content::Block::from_value(
                serde_json::to_value(block).unwrap_or(serde_json::Value::Null),
            )
        })
        .collect();
    crate::acp::common::session::begin_prompt_turn(state, store_session, blocks, owner).await
}

/// The model turn: long, and it awaits client messages
/// (`request_permission`). The connection processes messages on one task, so
/// the turn runs spawned, keeping the dispatch loop free to route the
/// client's replies.
#[allow(clippy::too_many_arguments)] // the turn's resolved inputs
pub(super) async fn run_model_turn_case(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    cx: Cx,
    wire_session: SessionId,
    store_session: crate::store::SessionId,
    owner: Arc<String>,
    turn: crate::store::Turn,
    actor_name: String,
    session_row: Option<crate::store::Session>,
    first_text: String,
    blocks_json: String,
    responder: Responder<PromptResponse>,
) -> Result<(), Error> {
    // The turn's cancellation token is registered before the turn is
    // spawned: the dispatch loop processes the prompt handler to
    // completion first, so any `session/cancel` that follows finds it.
    // The registration guard rides the spawned task, so the entry goes
    // away on every exit path.
    let request_cancellation = responder.cancellation();
    let (cancellation, registration) = negotiation.register_cancellation(&store_session);
    let runtime = turn_runtime(
        &negotiation,
        &cx,
        wire_session,
        cancellation.clone(),
    );
    // Bridge `$/cancel_request`: the SDK marks the prompt request's
    // cancellation before handlers run, so a watcher folds it into the
    // same token the other signals cancel.
    {
        let cancellation = cancellation.clone();
        cx.spawn(async move {
            request_cancellation.cancelled().await;
            cancellation.cancel();
            Ok(())
        })?;
    }
    cx.spawn(async move {
        let _registration = registration;
        match run_prompt_turn(
            state,
            runtime,
            store_session,
            turn,
            actor_name,
            session_row,
            first_text,
            blocks_json,
            owner,
        )
        .await
        {
            Ok(stop) => responder.respond(PromptResponse::new(match stop {
                crate::agent::TurnStop::EndTurn | crate::agent::TurnStop::ContextLength => {
                    StopReason::EndTurn
                }
                crate::agent::TurnStop::MaxTurnRequests => StopReason::MaxTurnRequests,
                crate::agent::TurnStop::Cancelled => StopReason::Cancelled,
            })),
            Err(err) => responder.respond_with_error(Error::internal_error().data(err)),
        }
    })
}

/// The `/fork` fallback for v1 clients: exact first-token match on
/// user-typed input. Creates the fork, seeds it, fires the
/// `session_forked` scripts, and reports the new session id in the parent
/// turn — no model request.
#[allow(clippy::too_many_arguments)] // the turn's resolved inputs
pub(super) async fn run_fork_case(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    cx: Cx,
    wire_session: SessionId,
    store_session: crate::store::SessionId,
    owner: Arc<String>,
    turn: crate::store::Turn,
    responder: Responder<PromptResponse>,
) -> Result<(), Error> {
    let access = fork_access(&state, connection_notices(&cx, &negotiation, "fork"));
    let fork = match fork::run_fork(&state, Arc::clone(&access), &store_session).await {
        Ok(fork) => fork,
        Err(err) => {
            state.db.release_lease(&store_session, &owner).await.ok();
            return responder.respond_with_error(Error::internal_error().data(err.to_string()));
        }
    };
    // Seeds are harness-authored static user-facing content with their own
    // messageId.
    let _ = fork::insert_fork_seed(&state.db, &fork.id, &store_session).await;

    // The parent-turn report: an agent message carrying the new session id
    // (wire form for the client).
    let wire_fork_id = wire_session_id(&fork.id).to_string();
    let report = fork::fork_report_message(&wire_fork_id, fork.fork_point_turn_id.as_ref());
    let message = append_assistant_text(&state, &turn.id, &report).await?;
    send_agent_message_chunk(&cx, &wire_session, &message.id, &report);

    // updatedAt each turn — the fork's report turn too.
    send_updated_at(&state, &cx, &wire_session, &store_session).await;
    state.db.release_lease(&store_session, &owner).await.ok();
    responder.respond(PromptResponse::new(StopReason::EndTurn))
}

/// `/!mcp.<server>.<tool> {json}` or `/!client.<surface>.<method> {json}`:
/// the user calls a tool directly. No model request, and no permission
/// prompt — the call is the user's own keystroke, so the pipeline has
/// nothing to ask. The result is reported as a tool call and stored in the
/// turn.
#[allow(clippy::too_many_arguments)] // the turn's resolved inputs
pub(super) async fn run_direct_case(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    cx: Cx,
    wire_session: SessionId,
    store_session: crate::store::SessionId,
    owner: Arc<String>,
    turn_id: crate::store::TurnId,
    input: crate::invokables::DirectInvocation,
    responder: Responder<PromptResponse>,
) -> Result<(), Error> {
    let call_id = format!("direct-{}", uuid::Uuid::new_v4());
    let arguments: serde_json::Value =
        serde_json::from_str(&input.arguments).unwrap_or(serde_json::Value::Null);
    let announce = ToolCall::new(
        ToolCallId::new(call_id.clone()),
        crate::invokables::titles::tool_call_title(&input.invokable, &arguments),
    )
    .name(input.invokable.clone())
    .kind(wire_kind(crate::invokables::titles::tool_kind(
        &input.invokable,
    )))
    .status(ToolCallStatus::InProgress)
    .raw_input(Some(arguments.clone()));
    let _ = cx.send_notification(SessionNotification::new(
        wire_session.clone(),
        SessionUpdate::ToolCall(announce),
    ));

    // Direct invocation may issue a request back to the client (a
    // client-capability tool), and awaiting it inside the dispatch callback
    // would deadlock the dispatch loop on its own response. Run the whole
    // effect on a spawned task, like a model turn, and respond when it
    // completes.
    let runtime = turn_runtime(
        &negotiation,
        &cx,
        wire_session.clone(),
        CancellationToken::new(),
    );
    cx.spawn(async move {
        match run_direct_invocation(
            state,
            runtime,
            turn_id,
            store_session,
            owner,
            call_id,
            input,
        )
        .await
        {
            Ok(()) => responder.respond(PromptResponse::new(StopReason::EndTurn)),
            Err(err) => responder.respond_with_error(Error::internal_error().data(err)),
        }
    })
}

/// A recognised `/mcp` or `/editor` subcommand: answer the turn with its
/// reply, republish the command listing (the tool surface may have changed
/// shape), and release the lease.
#[allow(clippy::too_many_arguments)] // the turn's resolved inputs
pub(super) async fn run_runtime_command_case(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    cx: Cx,
    wire_session: SessionId,
    store_session: crate::store::SessionId,
    owner: Arc<String>,
    turn_id: crate::store::TurnId,
    reply: String,
    responder: Responder<PromptResponse>,
) -> Result<(), Error> {
    let message = append_assistant_text(&state, &turn_id, &reply).await?;
    send_agent_message_chunk(&cx, &wire_session, &message.id, &reply);
    // The tool surface changed shape, so the command listing is republished
    // for the session.
    let commands = available_commands_update(&state, &negotiation).await;
    let _ = cx.send_notification(SessionNotification::new(wire_session.clone(), commands));
    state.db.release_lease(&store_session, &owner).await.ok();
    responder.respond(PromptResponse::new(StopReason::EndTurn))
}

/// A compaction-tagged prompt: intercepted — the `compaction_requested`
/// script event fires instead of a model turn in the main session.
#[allow(clippy::too_many_arguments)] // the turn's resolved inputs
pub(super) async fn run_compaction_case(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    cx: Cx,
    wire_session: SessionId,
    store_session: crate::store::SessionId,
    owner: Arc<String>,
    turn_id: crate::store::TurnId,
    first_text: String,
    actor_name: String,
    session_row: Option<crate::store::Session>,
    responder: Responder<PromptResponse>,
) -> Result<(), Error> {
    let size = session_row
        .as_ref()
        .and_then(|row| resolve_model_for(&state, row, None))
        .and_then(|model_ref| effective_context_window(&state, &model_ref))
        .unwrap_or(0);
    let (before, after) = compaction::run_compaction(
        &state,
        &store_session,
        &turn_id,
        &actor_name,
        size,
        connection_notices(&cx, &negotiation, "compaction"),
    )
    .await;

    // The in-band announcement: trigger and counts.
    let note = compaction::announcement(first_text.trim_start_matches('/'), before, after);
    let message = append_assistant_text(&state, &turn_id, &note).await?;
    send_agent_message_chunk(&cx, &wire_session, &message.id, &note);

    // The visible usage_update drop, carrying the session's cumulative cost
    // when known.
    if size > 0 {
        let session_usage = state
            .db
            .session_usage(&store_session)
            .await
            .map_err(|err| Error::internal_error().data(err.to_string()))?;
        let cost = (session_usage.cost_usd > 0.0).then_some(session_usage.cost_usd);
        let _ = cx.send_notification(SessionNotification::new(
            wire_session.clone(),
            SessionUpdate::UsageUpdate(usage_update(after, size, cost)),
        ));
    }

    // RFD-shaped compaction updates flow only to clients advertising the
    // capability.
    if negotiation.supports(UnstableFeature::SessionCompaction) {
        let update = CompactionUpdate::new(
            CompactionId::new(uuid::Uuid::new_v4().to_string()),
            CompactionStatus::Completed,
        )
        .summary(vec![ContentBlock::Text(TextContent::new(note.clone()))]);
        let _ = cx.send_notification(SessionNotification::new(
            wire_session.clone(),
            SessionUpdate::CompactionUpdate(update),
        ));
    }

    // updatedAt each turn — intercepted turns included.
    send_updated_at(&state, &cx, &wire_session, &store_session).await;
    state.db.release_lease(&store_session, &owner).await.ok();
    responder.respond(PromptResponse::new(StopReason::EndTurn))
}

/// `session/prompt`: acquire the lease, persist the user message, then
/// dispatch to one of the intercepted cases or the model turn.
#[tracing::instrument(skip_all, fields(version = "v1", session = %request.session_id))]
pub(super) async fn handle_prompt(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    owner: Arc<String>,
    cx: Cx,
    request: PromptRequest,
    responder: Responder<PromptResponse>,
) -> Result<(), Error> {
use agent_client_protocol::schema::v1::ContentBlock;

// The lease is the one-turn-at-a-time invariant, and the
// prompt turn and its user message are persisted up front.
let begun = match begin_prompt_turn(&state, &request, &owner)
    .await
{
    Ok(begun) => begun,
    Err(BeginError::Owned { holder }) => {
        return responder.respond_with_error(Error::internal_error().data(
            serde_json::json!({
                "error": "session-actively-owned",
                "owner": holder,
            }),
        ));
    }
    Err(BeginError::Store(err)) => {
        return responder.respond_with_error(Error::internal_error().data(err));
    }
};
let _lease_heartbeat = begun.heartbeat;
let session_id = begun.store_session;
let turn = begun.turn;
let blocks_json = begun.blocks_json;

// The first user text drives slash-command dispatch; a
// non-text first block yields the empty string and falls
// through to the model turn.
let first_text = request
    .prompt
    .first()
    .and_then(|block| match block {
        ContentBlock::Text(text) => Some(text.text.trim().to_owned()),
        _ => None,
    })
    .unwrap_or_default();

// Resolve the actor for this session (compaction's event
// payload and the model turn both need it).
let session_row = state.db.get_session(&session_id).await.ok().flatten();
let actor_name = session_row
    .as_ref()
    .map(|row| {
        session_actor(row, state.config().config.defaults.actor.as_deref())
    })
    .unwrap_or_else(|| "default".into());

// Which prompt case applies is a protocol-neutral decision,
// shared with v2 through acp::common.
match decide(
    &first_text,
    compaction::is_compaction_command(&state.definitions(), &first_text),
) {
    // The /fork fallback for v1 clients: exact first-token
    // match on user-typed input. Creates the fork, seeds it,
    // fires the session_forked scripts, and reports the new
    // session id in the parent turn — no model request.
    Dispatch::Fork => {
        return run_fork_case(
            state.clone(),
            negotiation.clone(),
            cx.clone(),
            request.session_id.clone(),
            session_id.clone(),
            owner.clone(),
            turn.clone(),
            responder,
        )
        .await;
    }
    // `/!mcp.<server>.<tool> {json}` or
    // `/!client.<surface>.<method> {json}`: the user calls a
    // tool directly. No model request, and no permission
    // prompt — the call is the user's own keystroke, so the
    // pipeline has nothing to ask.
    Dispatch::Direct(input) => {
        return run_direct_case(
            state.clone(),
            negotiation.clone(),
            cx.clone(),
            request.session_id.clone(),
            session_id.clone(),
            owner.clone(),
            turn.id.clone(),
            input,
            responder,
        )
        .await;
    }
    // Runtime availability commands: `/mcp` and `/editor`.
    // An unrecognised subcommand yields no reply and falls
    // through to normal processing.
    Dispatch::RuntimeCommand => {
        let reply = if first_text.starts_with("/editor") {
            editor_command(&negotiation, &first_text)
        } else {
            mcp_command(&negotiation, &first_text).await
        };
        if let Some(reply) = reply {
            return run_runtime_command_case(
                state.clone(),
                negotiation.clone(),
                cx.clone(),
                request.session_id.clone(),
                session_id.clone(),
                owner.clone(),
                turn.id.clone(),
                reply,
                responder,
            )
            .await;
        }
    }
    // `/context`: an ephemeral composition report. Exact
    // first-token match with no arguments; the command turn
    // is deleted before the lease is released.
    Dispatch::Context => {
        return super::context::run_context_case(
            state.clone(),
            negotiation.clone(),
            cx.clone(),
            request.session_id.clone(),
            session_id.clone(),
            owner.clone(),
            turn.id.clone(),
            actor_name.clone(),
            session_row.clone(),
            responder,
        )
        .await;
    }
    // Compaction-tagged prompts: intercepted — the
    // compaction_requested script event fires instead of a
    // model turn in the main session.
    Dispatch::Compaction => {
        return run_compaction_case(
            state.clone(),
            negotiation.clone(),
            cx.clone(),
            request.session_id.clone(),
            session_id.clone(),
            owner.clone(),
            turn.id.clone(),
            first_text.clone(),
            actor_name.clone(),
            session_row.clone(),
            responder,
        )
        .await;
    }
    Dispatch::Model => {}
}

// Nothing intercepted: run the model turn.
run_model_turn_case(
    state.clone(),
    negotiation.clone(),
    cx.clone(),
    request.session_id.clone(),
    session_id,
    owner.clone(),
    turn,
    actor_name,
    session_row,
    first_text,
    blocks_json,
    responder,
)
.await
}
