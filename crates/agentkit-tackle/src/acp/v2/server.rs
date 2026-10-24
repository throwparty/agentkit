//! The ACP v2 server: the per-connection handler builder and v2 method
//! registration.

use crate::state::TackleState;
use agent_client_protocol::schema::v2::{
    CloseSessionRequest, DeleteSessionRequest, InitializeRequest, ListSessionsRequest,
    LoginAuthRequest, LogoutAuthRequest, NewSessionRequest, SetSessionConfigOptionRequest,
};
use agent_client_protocol::role::acp::Client;
use agent_client_protocol::{Agent, V2ConnectionTo};
use std::sync::Arc;

/// Builds the configured v2 agent: one thin closure per ACP method, capturing
/// the shared state. The caller hands it to the version router.
pub(crate) fn agent(
    state: Arc<TackleState>,
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
                async move |request: NewSessionRequest,
                            responder,
                            _connection: V2ConnectionTo<Client>| {
                    super::sessions::handle_new_session(state.clone(), request, responder).await
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
                async move |request: CloseSessionRequest,
                            responder,
                            _connection: V2ConnectionTo<Client>| {
                    super::sessions::handle_close_session(state.clone(), request, responder).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                async move |request: DeleteSessionRequest,
                            responder,
                            _connection: V2ConnectionTo<Client>| {
                    super::sessions::handle_delete_session(state.clone(), request, responder).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                async move |request: SetSessionConfigOptionRequest,
                            responder,
                            _connection: V2ConnectionTo<Client>| {
                    super::sessions::handle_set_config(state.clone(), request, responder).await
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
}
