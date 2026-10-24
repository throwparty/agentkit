//! The `session/prompt` dispatch cases: the slash-command intercepts (`/fork`,
//! `/!`, `/mcp`, `/editor`, compaction) and the model turn, each lifted out of
//! the request handler so it reads as a short ladder.

use super::capabilities::UnstableFeature;
use super::commands::available_commands_update;
use super::negotiation::ConnectionNegotiation;
use super::notices::connection_notices;
use super::sink::usage_update;
use super::support::{
    append_assistant_text, send_agent_message_chunk, send_updated_at, turn_runtime, Cx,
};
use super::wire_kind;
use crate::agent::compaction;
use crate::agent::config_options::{effective_context_window, resolve_model_for};
use crate::agent::direct::run_direct_invocation;
use crate::agent::fork::{self, fork_access};
use crate::agent::prompt::run_prompt_turn;
use crate::state::{
    store_session_id, wire_session_id, LeaseHeartbeat, TackleState, LEASE_TTL_SECS,
};
use agent_client_protocol::schema::v1::{
    CompactionId, CompactionStatus, CompactionUpdate, ContentBlock, PromptRequest, PromptResponse,
    SessionId, SessionNotification, SessionUpdate, StopReason, TextContent, ToolCall, ToolCallId,
    ToolCallStatus,
};
use agent_client_protocol::{Error, Responder};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// The persisted state of a begun prompt turn, plus the lease heartbeat
/// that must outlive it.
pub(super) struct BegunTurn {
    pub store_session: crate::store::SessionId,
    pub turn: crate::store::Turn,
    pub blocks_json: String,
    pub heartbeat: LeaseHeartbeat,
}

/// Why a prompt turn could not begin.
pub(super) enum BeginError {
    /// Another connection holds the session's turn lease.
    Owned { holder: String },
    /// The store failed while acquiring the lease or persisting the turn.
    Store(String),
}

/// Acquires the one-turn-at-a-time lease and persists the prompt turn and
/// its user message. The returned heartbeat must be held for the turn's
/// lifetime; dropping it stops renewal.
pub(super) async fn begin_prompt_turn(
    state: &TackleState,
    request: &PromptRequest,
    owner: &str,
) -> Result<BegunTurn, BeginError> {
    let store_session = store_session_id(&request.session_id);
    let acquired = state
        .db
        .acquire_lease(&store_session, owner, LEASE_TTL_SECS)
        .await
        .map_err(|err| BeginError::Store(err.to_string()))?;
    if !acquired {
        let holder = state
            .db
            .lease_holder(&store_session)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "another connection".into());
        return Err(BeginError::Owned { holder });
    }
    let heartbeat = LeaseHeartbeat::spawn(
        Arc::clone(&state.db),
        store_session.clone(),
        owner.to_owned(),
    );
    // Persist the prompt turn + the user message (original content blocks,
    // stored as the JSON block array).
    let blocks_json = serde_json::to_string(&request.prompt).unwrap_or_else(|_| "[]".to_owned());
    let head = state
        .db
        .get_session(&store_session)
        .await
        .ok()
        .flatten()
        .and_then(|session| session.head_turn_id);
    let turn = state
        .db
        .append_turn(
            &store_session,
            head.as_ref(),
            crate::store::TurnKind::Interaction,
            None,
            crate::store::TurnUsage::default(),
        )
        .await
        .map_err(|err| BeginError::Store(err.to_string()))?;
    state
        .db
        .append_message(
            &turn.id,
            crate::store::Role::User,
            &blocks_json,
            None,
            None,
            None,
            None,
        )
        .await
        .map_err(|err| BeginError::Store(err.to_string()))?;
    Ok(BegunTurn {
        store_session,
        turn,
        blocks_json,
        heartbeat,
    })
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
    let runtime = turn_runtime(&negotiation, &cx, wire_session, CancellationToken::new());
    cx.spawn(async move {
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
