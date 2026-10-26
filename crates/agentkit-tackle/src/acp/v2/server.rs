//! The ACP v2 server: the per-connection handler builder and v2 method
//! registration.

use super::negotiation::ConnectionNegotiation;
use crate::state::TackleState;
use agent_client_protocol::role::acp::Client;
use agent_client_protocol::schema::v2::{
    CancelSessionNotification, CloseSessionRequest, DeleteSessionRequest, ForkSessionRequest,
    InitializeRequest, ListSessionsRequest, LoginAuthRequest, LogoutAuthRequest, NewSessionRequest,
    PromptRequest, ResumeSessionRequest, SetSessionConfigOptionRequest,
};
use agent_client_protocol::{Agent, V2ConnectionTo};
use std::sync::Arc;

/// Builds the configured v2 agent: one thin closure per ACP method, capturing
/// the shared state and this connection's negotiation and lease-owner id.
pub(crate) fn agent(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    owner: Arc<String>,
) -> impl agent_client_protocol::ConnectTo<Client> {
    Agent
        .v2()
        .name("tackle")
        .on_receive_request(
            {
                let state = state.clone();
                async move |request: InitializeRequest,
                            responder,
                            _connection: V2ConnectionTo<Client>| {
                    super::initialize::handle_initialize(state.clone(), request, responder).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: LoginAuthRequest,
                        responder,
                        _connection: V2ConnectionTo<Client>| {
                super::initialize::handle_login(request, responder)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: LogoutAuthRequest,
                        responder,
                        _connection: V2ConnectionTo<Client>| {
                super::initialize::handle_logout(request, responder)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                let negotiation = negotiation.clone();
                async move |request: NewSessionRequest,
                            responder,
                            connection: V2ConnectionTo<Client>| {
                    super::sessions::handle_new_session(
                        state.clone(),
                        negotiation.clone(),
                        connection,
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
                async move |request: ListSessionsRequest,
                            responder,
                            _connection: V2ConnectionTo<Client>| {
                    super::sessions::handle_list_sessions(state.clone(), request, responder).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                let negotiation = negotiation.clone();
                let owner = owner.clone();
                async move |request: PromptRequest,
                            responder,
                            connection: V2ConnectionTo<Client>| {
                    super::prompt::handle_prompt(
                        state.clone(),
                        negotiation.clone(),
                        connection,
                        owner.clone(),
                        request,
                        responder,
                    )
                    .await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let negotiation = negotiation.clone();
                async move |notification: CancelSessionNotification, _connection: V2ConnectionTo<Client>| {
                    let store_session = super::store_session_id(&notification.session_id);
                    tracing::info!(
                        version = "v2",
                        session = %store_session,
                        "cancelling the session's active turn"
                    );
                    negotiation.cancel(&store_session);
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                let negotiation = negotiation.clone();
                async move |request: ResumeSessionRequest,
                            responder,
                            connection: V2ConnectionTo<Client>| {
                    super::sessions::handle_resume_session(
                        state.clone(),
                        negotiation.clone(),
                        connection,
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
                async move |request: CloseSessionRequest,
                            responder,
                            _connection: V2ConnectionTo<Client>| {
                    super::sessions::handle_close_session(
                        state.clone(),
                        negotiation.clone(),
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
                async move |request: DeleteSessionRequest,
                            responder,
                            _connection: V2ConnectionTo<Client>| {
                    super::sessions::handle_delete_session(
                        state.clone(),
                        negotiation.clone(),
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
                async move |request: ForkSessionRequest,
                            responder,
                            connection: V2ConnectionTo<Client>| {
                    super::sessions::handle_fork_session(
                        state.clone(),
                        negotiation.clone(),
                        connection,
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
                async move |request: SetSessionConfigOptionRequest,
                            responder,
                            connection: V2ConnectionTo<Client>| {
                    super::sessions::handle_set_config(state.clone(), connection, request, responder)
                        .await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
}
