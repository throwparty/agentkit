//! The ACP v1 server: the per-connection handler builder and dispatch.

use super::commands::{editor_command, mcp_command};
use super::negotiation::ConnectionNegotiation;
use crate::agent::compaction;
use crate::agent::config_options::session_actor;
use crate::acp::common::dispatch::{decide, Dispatch};
use crate::state::TackleState;
use agent_client_protocol::schema::v1::{
    InitializeRequest, LoadSessionRequest, NewSessionRequest, ResumeSessionRequest,
};
use agent_client_protocol::{Agent, Error};
use std::sync::Arc;

/// Builds the configured v1 agent: one thin closure per ACP method, capturing
/// the shared state and this connection's negotiation and lease-owner id. The
/// caller connects it to a transport, or hands it to the version router.
pub(crate) fn agent(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    owner: Arc<String>,
) -> impl agent_client_protocol::ConnectTo<agent_client_protocol::role::acp::Client> {
    Agent
        .builder()
        .name("tackle")
        .on_receive_request(
            {
                let state = state.clone();
                let negotiation = negotiation.clone();
                async move |request: InitializeRequest, responder, _cx| {
                    super::initialize::handle_initialize(state.clone(), negotiation.clone(), request, responder)
                        .await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: agent_client_protocol::schema::v1::AuthenticateRequest,
                        responder,
                        _cx| { super::initialize::handle_authenticate(responder) },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                let negotiation = negotiation.clone();
                async move |request: NewSessionRequest, responder, cx| {
                    super::sessions::handle_new_session(state.clone(), negotiation.clone(), cx, request, responder)
                        .await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                async move |request: crate::agent_client_protocol::schema::v1::ListSessionsRequest,
                            responder,
                            _cx| {
                    super::sessions::handle_list_sessions(state.clone(), request, responder).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                let negotiation = negotiation.clone();
                async move |request: agent_client_protocol::schema::v1::CloseSessionRequest,
                            responder,
                            cx| {
                    super::sessions::handle_close_session(
                        state.clone(),
                        negotiation.clone(),
                        cx,
                        request,
                        responder,
                    )
                    .await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                let negotiation = negotiation.clone();
                let owner = owner.clone();
                async move |request: agent_client_protocol::schema::v1::PromptRequest,
                            responder,
                            cx| {
                    use agent_client_protocol::schema::v1::ContentBlock;

                    // The lease is the one-turn-at-a-time invariant, and the
                    // prompt turn and its user message are persisted up front.
                    let begun = match super::prompt::begin_prompt_turn(&state, &request, &owner)
                        .await
                    {
                        Ok(begun) => begun,
                        Err(super::prompt::BeginError::Owned { holder }) => {
                            return responder.respond_with_error(Error::internal_error().data(
                                serde_json::json!({
                                    "error": "session-actively-owned",
                                    "owner": holder,
                                }),
                            ));
                        }
                        Err(super::prompt::BeginError::Store(err)) => {
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
                            return super::prompt::run_fork_case(
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
                            return super::prompt::run_direct_case(
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
                                return super::prompt::run_runtime_command_case(
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
                            return super::prompt::run_compaction_case(
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
                    super::prompt::run_model_turn_case(
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
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                async move |request: agent_client_protocol::schema::v1::DeleteSessionRequest,
                            responder,
                            _cx| {
                    super::sessions::handle_delete_session(state.clone(), request, responder).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                let negotiation = negotiation.clone();
                async move |request: LoadSessionRequest, responder, cx| {
                    super::sessions::handle_load_session(state.clone(), negotiation.clone(), cx, request, responder)
                        .await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                let negotiation = negotiation.clone();
                async move |request: ResumeSessionRequest, responder, cx| {
                    super::sessions::handle_resume_session(
                        state.clone(),
                        negotiation.clone(),
                        cx,
                        request,
                        responder,
                    )
                    .await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                let negotiation = negotiation.clone();
                async move |request: agent_client_protocol::schema::v1::SetSessionConfigOptionRequest,
                            responder,
                            cx| {
                    super::sessions::handle_set_config(state.clone(), negotiation.clone(), cx, request, responder)
                        .await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        // The RFD path for forking: session/fork, served to clients that call
        // it; the fork capability advertisement is itself gated.
        .on_receive_request(
            {
                let state = state.clone();
                let negotiation = negotiation.clone();
                async move |request: agent_client_protocol::schema::v1::ForkSessionRequest,
                            responder,
                            cx| {
                    super::sessions::handle_fork_session(state.clone(), negotiation.clone(), cx, request, responder)
                        .await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        // `session/cancel` is a notification (no response). It cancels the
        // session's in-flight turn, if any; an idle or unknown session is a
        // no-op.
        .on_receive_notification(
            {
                let negotiation = negotiation.clone();
                async move |notification: agent_client_protocol::schema::v1::CancelNotification,
                            _cx| {
                    let store_session = crate::state::store_session_id(&notification.session_id);
                    negotiation.cancel(&store_session);
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
}
