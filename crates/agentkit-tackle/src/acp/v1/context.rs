//! The ephemeral `/context` command: build the same system prompt, tool
//! catalogue, and prior context a model turn would receive, render the
//! estimate, stream it, and delete the command turn so neither the command
//! nor the reply ever enters the session.

use super::negotiation::ConnectionNegotiation;
use super::support::{send_agent_message_chunk, turn_runtime, Cx};
use crate::agent::compaction;
use crate::agent::context_report::{self, ReportInputs};
use crate::agent::prompt::{build_registry_and_system, resolve_model_and_window, tool_names_for};
use crate::state::TackleState;
use agent_client_protocol::schema::v1::{PromptResponse, SessionId, StopReason};
use agent_client_protocol::{Error, Responder};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Handles an exact `/context` prompt: no model request, no persisted
/// message, and the prompt turn is deleted before the lease is released.
#[allow(clippy::too_many_arguments)] // the turn's resolved inputs
pub(super) async fn run_context_case(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    cx: Cx,
    wire_session: SessionId,
    store_session: crate::store::SessionId,
    owner: Arc<String>,
    turn_id: crate::store::TurnId,
    actor_name: String,
    session_row: Option<crate::store::Session>,
    responder: Responder<PromptResponse>,
) -> Result<(), Error> {
    let result = build_reply(
        &state,
        &negotiation,
        &cx,
        &wire_session,
        &store_session,
        &turn_id,
        &actor_name,
        session_row.as_ref(),
    )
    .await;

    match result {
        Ok((text, message_id)) => {
            send_agent_message_chunk(&cx, &wire_session, &message_id, &text);
            let deleted = state.db.delete_turn(&turn_id).await;
            state.db.release_lease(&store_session, &owner).await.ok();
            match deleted {
                Ok(()) => responder.respond(PromptResponse::new(StopReason::EndTurn)),
                Err(err) => {
                    responder.respond_with_error(Error::internal_error().data(err.to_string()))
                }
            }
        }
        Err(message) => {
            state.db.delete_turn(&turn_id).await.ok();
            state.db.release_lease(&store_session, &owner).await.ok();
            responder.respond_with_error(Error::internal_error().data(message))
        }
    }
}

#[allow(clippy::too_many_arguments)] // the report's resolved inputs
async fn build_reply(
    state: &TackleState,
    negotiation: &Arc<ConnectionNegotiation>,
    cx: &Cx,
    wire_session: &SessionId,
    store_session: &crate::store::SessionId,
    turn_id: &crate::store::TurnId,
    actor_name: &str,
    session_row: Option<&crate::store::Session>,
) -> Result<(String, String), String> {
    let (model_ref, window) = resolve_model_and_window(state, session_row).await?;
    let (scheme, context) = tool_names_for(&state.config().config, &model_ref)?;
    let runtime = turn_runtime(negotiation, cx, wire_session.clone(), CancellationToken::new());
    let (_, rendered, system) = build_registry_and_system(
        &state.definitions(),
        actor_name,
        &runtime.pool,
        &runtime.client_tools,
        scheme,
        context,
    )
    .await?;
    let tools = rendered.tools;
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
    Ok((text, uuid::Uuid::new_v4().to_string()))
}
