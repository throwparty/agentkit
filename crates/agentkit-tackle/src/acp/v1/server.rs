//! The ACP v1 server: the per-connection handler builder and dispatch.

use super::negotiation::ConnectionNegotiation;
use crate::state::TackleState;
use agent_client_protocol::schema::v1::{
    InitializeRequest, LoadSessionRequest, NewSessionRequest, ResumeSessionRequest,
};
use agent_client_protocol::Agent;
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
                    super::prompt::handle_prompt(
                        state.clone(),
                        negotiation.clone(),
                        owner.clone(),
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
