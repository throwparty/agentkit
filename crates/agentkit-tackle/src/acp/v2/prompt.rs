//! The v2 `session/prompt` handler: insert the user message and respond with
//! its id immediately, then drive the turn and the foreground state machine on
//! a spawned task.

use super::negotiation::ConnectionNegotiation;
use crate::acp::common::content::Block;
use crate::acp::common::dispatch::{decide, Dispatch};
use crate::acp::common::messages::append_assistant_text;
use crate::acp::common::session::{begin_prompt_turn, BeginError, BegunTurn};
use crate::agent::config_options::{effective_context_window, resolve_model_for, session_actor};
use crate::agent::context_report::{self, ReportInputs};
use crate::agent::fork::{self, fork_access};
use crate::agent::prompt::{build_registry_and_system, resolve_model_and_window, run_prompt_turn};
use crate::agent::runtime::TurnRuntime;
use crate::agent::{compaction, TurnStop};
use crate::invokables::client_tools::{ClientToolExecutor, Executed};
use std::future::Future;
use std::pin::Pin;
use crate::scripts::host::NoticeSink;
use crate::state::TackleState;
use crate::store::SessionId;
use agent_client_protocol::role::acp::Client;
use agent_client_protocol::schema::v2::{
    AgentMessage, AvailableCommandsUpdate, CompactionId, CompactionStatus, CompactionUpdate,
    ContentBlock, Cost, ErrorStopReason, IdleStateUpdate, MessageId, PromptRequest, PromptResponse,
    RunningStateUpdate, SessionUpdate, StateUpdate, StopReason, TextContent,
    UpdateSessionNotification, UsageUpdate, UserMessage,
};
use agent_client_protocol::{Error, Responder, V2ConnectionTo};
use std::sync::Arc;

/// `session/prompt`: the insertion response is the user message id; the turn
/// runs detached, reporting `running` then a terminal `idle`.
#[tracing::instrument(skip_all, fields(version = "v2", session = %request.session_id))]
pub(super) async fn handle_prompt(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    cx: V2ConnectionTo<Client>,
    owner: Arc<String>,
    request: PromptRequest,
    responder: Responder<PromptResponse>,
) -> Result<(), Error> {
    let session_id = super::store_session_id(&request.session_id);
    let blocks: Vec<Block> = request
        .prompt
        .iter()
        .map(|block| {
            Block::from_value(serde_json::to_value(block).unwrap_or(serde_json::Value::Null))
        })
        .collect();
    let request_cancellation = responder.cancellation();
    if matches!(
        futures::poll!(Box::pin(request_cancellation.cancelled())),
        std::task::Poll::Ready(())
    ) {
        return responder.respond_with_error(Error::request_cancelled());
    }

    match begin_prompt_turn(
        &state,
        session_id.clone(),
        blocks.clone(),
        owner.as_str(),
    )
    .await
    {
        Ok(begun) => {
            run_prompt_after_begin(state, negotiation, cx, owner, request, responder, begun).await
        }
        Err(BeginError::Owned { holder }) if holder == owner.as_str() => {
            // v2 accepts a prompt before the previous turn ends. Wait for
            // this connection's active turn to finish on a spawned task, so
            // the dispatch loop stays free to route its permission replies.
            let state = Arc::clone(&state);
            let negotiation = Arc::clone(&negotiation);
            let turn_cx = cx.clone();
            let owner = Arc::clone(&owner);
            tracing::info!(
                version = "v2",
                session = %session_id,
                "queued prompt waits for the active turn"
            );
            cx.spawn(async move {
                let deadline =
                    tokio::time::Instant::now() + std::time::Duration::from_secs(60);
                loop {
                    if matches!(
                        futures::poll!(Box::pin(request_cancellation.cancelled())),
                        std::task::Poll::Ready(())
                    ) {
                        return responder.respond_with_error(Error::request_cancelled());
                    }
                    match begin_prompt_turn(
                        &state,
                        session_id.clone(),
                        blocks.clone(),
                        owner.as_str(),
                    )
                    .await
                    {
                        Ok(begun) => {
                            return run_prompt_after_begin(
                                state,
                                negotiation,
                                turn_cx,
                                owner,
                                request,
                                responder,
                                begun,
                            )
                            .await
                        }
                        Err(BeginError::Owned { holder }) if holder == owner.as_str() => {}
                        Err(BeginError::Owned { holder }) => {
                            tracing::warn!(
                                version = "v2",
                                session = %session_id,
                                holder = %holder,
                                "prompt refused: session actively owned"
                            );
                            return responder.respond_with_error(
                                Error::internal_error().data(serde_json::json!({
                                    "error": "session-actively-owned",
                                    "owner": holder,
                                })),
                            )
                        }
                        Err(BeginError::Store(err)) => {
                            return responder
                                .respond_with_error(Error::internal_error().data(err))
                        }
                    }
                    if tokio::time::Instant::now() >= deadline {
                        tracing::warn!(
                            version = "v2",
                            session = %session_id,
                            "queued prompt timed out waiting for the active turn"
                        );
                        return responder.respond_with_error(
                            Error::internal_error().data(serde_json::json!({
                                "error": "session-busy",
                                "reason": "timed out waiting for the active turn",
                            })),
                        );
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
            })?;
            Ok(())
        }
        Err(BeginError::Owned { holder }) => {
            tracing::warn!(
                version = "v2",
                session = %session_id,
                holder = %holder,
                "prompt refused: session actively owned"
            );
            responder.respond_with_error(Error::internal_error().data(serde_json::json!({
                "error": "session-actively-owned",
                "owner": holder,
            })))
        }
        Err(BeginError::Store(err)) => {
            responder.respond_with_error(Error::internal_error().data(err))
        }
    }
}

async fn run_prompt_after_begin(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    cx: V2ConnectionTo<Client>,
    owner: Arc<String>,
    request: PromptRequest,
    responder: Responder<PromptResponse>,
    begun: BegunTurn,
) -> Result<(), Error> {
    let prompt_blocks = request.prompt.clone();
    let first_text = request
        .prompt
        .iter()
        .find_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.trim().to_owned()),
            _ => None,
        })
        .unwrap_or_default();
    let _heartbeat = begun.heartbeat;
    let user_message_id = begun.user_message_id;
    let turn = begun.turn;
    let store_session = begun.store_session;
    let blocks_json = begun.blocks_json;

    let session_row = state.db.get_session(&store_session).await.ok().flatten();
    let actor_name = session_row
        .as_ref()
        .map(|row| session_actor(row, state.config().config.defaults.actor.as_deref()))
        .unwrap_or_else(|| "default".into());

    let request_cancellation = responder.cancellation();
    let (cancellation, registration) = negotiation.register_cancellation(&store_session);
    {
        let cancellation = cancellation.clone();
        let cancel_session = store_session.clone();
        cx.spawn(async move {
            request_cancellation.cancelled().await;
            tracing::info!(
                version = "v2",
                session = %cancel_session,
                "prompt cancellation requested"
            );
            cancellation.cancel();
            Ok(())
        })?;
    }

    // The acceptance receipt: the persisted user message id, sent at
    // insertion time rather than at turn end.
    responder.respond(PromptResponse::new(user_message_id.clone()))?;
    cx.send_notification(UpdateSessionNotification::new(
        request.session_id.clone(),
        SessionUpdate::UserMessage(
            UserMessage::new(agent_client_protocol::schema::v2::MessageId::new(user_message_id))
                .content(prompt_blocks),
        ),
    ))
    .map_err(|err| Error::internal_error().data(err.to_string()))?;

    let is_compaction =
        compaction::is_compaction_command(&state.definitions(), &first_text);
    let dispatch = decide(&first_text, is_compaction);
    let context_size = session_row
        .as_ref()
        .and_then(|row| resolve_model_for(&state, row, None))
        .and_then(|model| effective_context_window(&state, &model))
        .unwrap_or(0);

    let wire_session = super::wire_session_id(&store_session);
    let runtime = TurnRuntime {
        cancellation: cancellation.clone(),
        pool: negotiation.mcp_pool(),
        sink: Arc::new(super::sink::ConnectionTurnSink::new(cx.clone())),
        requester: Arc::new(super::permission::AcpPermissionRequester {
            cx: cx.clone(),
            session_id: wire_session.clone(),
        }),
        executor: Arc::new(NoClientTools),
        tracker: negotiation.clone(),
        compaction_notices: Arc::new(super::notices::ConnectionNotices {
            cx: cx.clone(),
            source: "compaction",
        }),
        titling_notices: Arc::new(super::notices::ConnectionNotices {
            cx: cx.clone(),
            source: "titling",
        }),
        client_tools: Vec::new(),
    };

    let turn_cx = cx.clone();
    cx.spawn(async move {
        // The heartbeat must outlive the turn, not just this function; it
        // returns as soon as the turn is spawned.
        let _heartbeat = _heartbeat;
        let _registration = registration;
        super::send_state(
            &turn_cx,
            &wire_session,
            StateUpdate::Running(RunningStateUpdate::new()),
        );
        let outcome = match dispatch {
            Dispatch::Fork => {
                let result = run_fork(
                    &state,
                    &turn_cx,
                    &wire_session,
                    &store_session,
                    &turn.id,
                )
                .await;
                state.db.release_lease(&store_session, &owner).await.ok();
                match result {
                    Ok(()) => Ok(TurnStop::EndTurn),
                    Err(err) => Err(err),
                }
            }
            Dispatch::Compaction => {
                let result = run_compaction(
                    &state,
                    &turn_cx,
                    &wire_session,
                    &store_session,
                    &turn.id,
                    &first_text,
                    &actor_name,
                    context_size,
                )
                .await;
                // The model turn releases its lease in `run_prompt_turn`; the
                // intercepted compaction path must do the same or the next
                // prompt sees the session as actively owned.
                state.db.release_lease(&store_session, &owner).await.ok();
                match result {
                    Ok(()) => Ok(TurnStop::EndTurn),
                    Err(err) => Err(err),
                }
            }
            Dispatch::Direct(input) => {
                let call_id = format!("direct-{}", uuid::Uuid::new_v4());
                let arguments: serde_json::Value =
                    serde_json::from_str(&input.arguments).unwrap_or(serde_json::Value::Null);
                runtime.sink.tool_call(
                    &store_session,
                    crate::agent::sink::ToolCall {
                        id: call_id.clone(),
                        title: crate::invokables::titles::tool_call_title(
                            &input.invokable,
                            &arguments,
                        ),
                        name: input.invokable.clone(),
                        kind: crate::invokables::titles::tool_kind(&input.invokable),
                        status: crate::agent::sink::ToolCallStatus::InProgress,
                        arguments,
                    },
                );
                crate::agent::direct::run_direct_invocation(
                    state,
                    runtime,
                    turn.id,
                    store_session,
                    owner,
                    call_id,
                    input,
                )
                .await
                .map(|()| TurnStop::EndTurn)
            }
            Dispatch::Context => {
                let result = run_context(
                    &state,
                    &turn_cx,
                    &wire_session,
                    &store_session,
                    &turn.id,
                    &actor_name,
                    session_row.as_ref(),
                    &runtime,
                )
                .await;
                state.db.release_lease(&store_session, &owner).await.ok();
                result.map(|()| TurnStop::EndTurn)
            }
            Dispatch::RuntimeCommand => {
                let reply = if first_text.starts_with("/editor") {
                    Some(
                        "The /editor command is not available under ACP v2; there is no client \
                         filesystem or terminal surface."
                            .to_owned(),
                    )
                } else {
                    super::commands::mcp_command(&negotiation, &first_text).await
                };
                match reply {
                    Some(reply) => {
                        let result = run_runtime_command(
                            &state,
                            &negotiation,
                            &turn_cx,
                            &wire_session,
                            &turn.id,
                            &reply,
                        )
                        .await;
                        state.db.release_lease(&store_session, &owner).await.ok();
                        result.map(|()| TurnStop::EndTurn)
                    }
                    None => {
                        run_prompt_turn(
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
                    }
                }
            }
            // Only an ordinary prompt reaches the model. Every intercept is
            // an explicit arm above; a new Dispatch variant fails compilation.
            Dispatch::Model => {
                run_prompt_turn(
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
            }
        };
        let stop = match outcome {
            Ok(TurnStop::EndTurn | TurnStop::ContextLength) => StopReason::EndTurn,
            Ok(TurnStop::MaxTurnRequests) => StopReason::MaxTurnRequests,
            Ok(TurnStop::Cancelled) => StopReason::Cancelled,
            Err(_) if cancellation.is_cancelled() => StopReason::Cancelled,
            Err(err) => {
                tracing::warn!(
                    version = "v2",
                    session = %wire_session,
                    error = %err,
                    "v2 turn failed"
                );
                StopReason::Error(ErrorStopReason::new().error(
                    agent_client_protocol::schema::v2::Error::internal_error().data(err),
                ))
            }
        };
        tracing::info!(
            version = "v2",
            session = %wire_session,
            outcome = ?stop,
            "v2 turn finished"
        );
        super::send_state(
            &turn_cx,
            &wire_session,
            StateUpdate::Idle(IdleStateUpdate::new().stop_reason(Some(stop))),
        );
        Ok(())
    })
}

/// ACP v2 removed the client filesystem and terminal surface, so the catalog
/// is empty and this is never reached.
struct NoClientTools;

impl ClientToolExecutor for NoClientTools {
    fn execute(
        &self,
        _session: &SessionId,
        _invokable: &str,
        _arguments: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<Executed, String>> + Send + '_>> {
        Box::pin(async { Err("no client tools under ACP v2".to_owned()) })
    }
}

/// A whole-message upsert's content patch: omit it, clear it, or replace it.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // the patch surface supports omit/clear; the live path replaces
pub(super) enum MessagePatch {
    Omit,
    Clear,
    Replace(Vec<ContentBlock>),
}

/// Builds a v2 `agent_message` upsert with the requested patch semantics.
fn message_upsert(message_id: MessageId, patch: MessagePatch) -> AgentMessage {
    let message = AgentMessage::new(message_id);
    match patch {
        MessagePatch::Omit => message,
        MessagePatch::Clear => {
            message.content(agent_client_protocol::schema::MaybeUndefined::<Vec<ContentBlock>>::Null)
        }
        MessagePatch::Replace(blocks) => message.content(blocks),
    }
}

/// Announces one whole agent message (the v2 upsert shape) under its id.
fn announce_agent_message(
    cx: &V2ConnectionTo<Client>,
    wire_session: &agent_client_protocol::schema::v2::SessionId,
    message_id: MessageId,
    blocks: Vec<ContentBlock>,
) {
    let _ = cx.send_notification(UpdateSessionNotification::new(
        wire_session.clone(),
        SessionUpdate::AgentMessage(message_upsert(message_id, MessagePatch::Replace(blocks))),
    ));
}

/// Announces a stored assistant message, preserving its stored id and
/// content blocks.
fn announce_stored_message(
    cx: &V2ConnectionTo<Client>,
    wire_session: &agent_client_protocol::schema::v2::SessionId,
    message: &crate::store::Message,
) {
    let blocks = serde_json::from_str::<Vec<ContentBlock>>(&message.content).unwrap_or_default();
    announce_agent_message(
        cx,
        wire_session,
        MessageId::new(message.id.clone()),
        blocks,
    );
}

/// The ephemeral `/context` report: build the same system prompt, tool
/// catalogue, and prior context a model turn would receive, send the estimate,
/// and delete the command turn so it never enters the session.
#[allow(clippy::too_many_arguments)]
async fn run_context(
    state: &Arc<TackleState>,
    cx: &V2ConnectionTo<Client>,
    wire_session: &agent_client_protocol::schema::v2::SessionId,
    store_session: &SessionId,
    turn_id: &crate::store::TurnId,
    actor_name: &str,
    session_row: Option<&crate::store::Session>,
    runtime: &TurnRuntime,
) -> Result<(), String> {
    let (model_ref, window) = resolve_model_and_window(state, session_row).await?;
    let (_, tools, system) = build_registry_and_system(
        &state.definitions(),
        actor_name,
        &runtime.pool,
        &runtime.client_tools,
    )
    .await;
    let assembled = state
        .db
        .assemble_context(store_session)
        .await
        .map_err(|err| err.to_string())?;
    let prior = compaction::prior_messages_tagged(&assembled, turn_id);
    let usage = state
        .db
        .session_usage(store_session)
        .await
        .map_err(|err| err.to_string())?;
    let cost_usd = (usage.cost_usd > 0.0).then_some(usage.cost_usd);
    let text = context_report::render(&ReportInputs {
        model: &model_ref,
        system: &system,
        tools: &tools,
        prior: &prior,
        window,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cost_usd,
    });
    let message_id = uuid::Uuid::new_v4().to_string();
    announce_agent_message(
        cx,
        wire_session,
        MessageId::new(message_id),
        vec![ContentBlock::Text(TextContent::new(text))],
    );
    state
        .db
        .delete_turn(turn_id)
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

/// A `/mcp` or `/editor` reply: store it in the turn, send it, and republish
/// the command list because the tool surface may have changed shape.
async fn run_runtime_command(
    state: &Arc<TackleState>,
    negotiation: &Arc<ConnectionNegotiation>,
    cx: &V2ConnectionTo<Client>,
    wire_session: &agent_client_protocol::schema::v2::SessionId,
    turn_id: &crate::store::TurnId,
    reply: &str,
) -> Result<(), String> {
    let message = append_assistant_text(state, turn_id, reply)
        .await
        .map_err(|err| err.to_string())?;
    announce_stored_message(cx, wire_session, &message);
    let commands = super::commands::available_commands(state, negotiation).await;
    let _ = cx.send_notification(UpdateSessionNotification::new(
        wire_session.clone(),
        SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(commands)),
    ));
    Ok(())
}

/// The `/fork` fallback: create the fork, seed it, and report the new
/// session id in the parent turn.
async fn run_fork(
    state: &Arc<TackleState>,
    cx: &V2ConnectionTo<Client>,
    wire_session: &agent_client_protocol::schema::v2::SessionId,
    store_session: &SessionId,
    turn_id: &crate::store::TurnId,
) -> Result<(), String> {
    let access = fork_access(
        state,
        Arc::new(super::notices::ConnectionNotices {
            cx: cx.clone(),
            source: "fork",
        }),
    );
    let fork = fork::run_fork(state, access, store_session)
        .await
        .map_err(|err| err.to_string())?;
    let _ = fork::insert_fork_seed(&state.db, &fork.id, store_session).await;

    let wire_fork_id = super::wire_session_id(&fork.id).to_string();
    let report = fork::fork_report_message(&wire_fork_id, fork.fork_point_turn_id.as_ref());
    let message = append_assistant_text(state, turn_id, &report)
        .await
        .map_err(|err| err.to_string())?;
    announce_stored_message(cx, wire_session, &message);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_compaction(
    state: &Arc<TackleState>,
    cx: &V2ConnectionTo<Client>,
    wire_session: &agent_client_protocol::schema::v2::SessionId,
    store_session: &SessionId,
    turn_id: &crate::store::TurnId,
    first_text: &str,
    actor_name: &str,
    context_size: u64,
) -> Result<(), String> {
    let notices: Arc<dyn NoticeSink> = Arc::new(super::notices::ConnectionNotices {
        cx: cx.clone(),
        source: "compaction",
    });
    let (before, after) = compaction::run_compaction(
        state,
        store_session,
        turn_id,
        actor_name,
        context_size,
        notices,
    )
    .await;
    let note = compaction::announcement(first_text.trim_start_matches('/'), before, after);
    let message = append_assistant_text(state, turn_id, &note)
        .await
        .map_err(|err| err.to_string())?;
    announce_stored_message(cx, wire_session, &message);
    if context_size > 0 {
        let session_usage = state
            .db
            .session_usage(store_session)
            .await
            .map_err(|err| err.to_string())?;
        let cost = (session_usage.cost_usd > 0.0).then_some(session_usage.cost_usd);
        let update = UsageUpdate::new(after, context_size);
        let update = match cost {
            Some(amount) => update.cost(Cost::new(amount, "USD")),
            None => update,
        };
        let _ = cx.send_notification(UpdateSessionNotification::new(
            wire_session.clone(),
            SessionUpdate::UsageUpdate(update),
        ));
    }
    let update = CompactionUpdate::new(
        CompactionId::new(uuid::Uuid::new_v4().to_string()),
        CompactionStatus::Completed,
    )
    .summary(vec![ContentBlock::Text(TextContent::new(note))]);
    let _ = cx.send_notification(UpdateSessionNotification::new(
        wire_session.clone(),
        SessionUpdate::CompactionUpdate(update),
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(patch: MessagePatch) -> serde_json::Value {
        let notification = UpdateSessionNotification::new(
            agent_client_protocol::schema::v2::SessionId::new("s"),
            SessionUpdate::AgentMessage(message_upsert(MessageId::new("m"), patch)),
        );
        serde_json::to_value(notification).expect("serialises")
    }

    #[test]
    fn an_omitted_content_patch_leaves_the_field_out() {
        let update = update(MessagePatch::Omit);
        assert!(
            update["update"].get("content").is_none(),
            "omit leaves content undefined: {update}"
        );
    }

    #[test]
    fn a_null_content_patch_clears_the_field() {
        let update = update(MessagePatch::Clear);
        assert!(
            update["update"]["content"].is_null(),
            "null clears content: {update}"
        );
    }

    #[test]
    fn a_replace_content_patch_carries_the_blocks() {
        let update = update(MessagePatch::Replace(vec![ContentBlock::Text(
            TextContent::new("replacement"),
        )]));
        assert_eq!(
            update["update"]["content"][0]["text"],
            "replacement",
            "replace carries the new blocks: {update}"
        );
    }
}
