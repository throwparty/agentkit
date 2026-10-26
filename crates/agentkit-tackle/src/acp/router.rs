//! The version router: one connection, one negotiated protocol version.

use super::common::connection::ConnectionSetup;
use super::v1;
use super::v2;
use crate::state::TackleState;
use agent_client_protocol::ConnectTo as _;
use std::sync::Arc;

/// Runs one connection through the version router: the initial `initialize`
/// selects the protocol implementation, and the connection is forwarded to it.
pub(crate) async fn run_acp_over<T>(
    state: Arc<TackleState>,
    setup: ConnectionSetup,
    transport: T,
) -> agent_client_protocol::Result<()>
where
    T: agent_client_protocol::ConnectTo<agent_client_protocol::role::acp::Agent>,
{
    // The connection's lease-owner id: turns acquire it for their lifetime;
    // released when the turn responds.
    let owner = Arc::new(format!("conn-{}", uuid::Uuid::new_v4()));
    let negotiation = Arc::new(v1::ConnectionNegotiation::new(
        v1::client_tools::EditorToggles {
            fs: setup.editor_fs,
            terminal: setup.editor_terminal,
        },
        setup.pool,
    ));
    let v2_negotiation = Arc::new(v2::negotiation::ConnectionNegotiation::with_pool(
        negotiation.mcp_pool(),
    ));
    agent_client_protocol::Agent
        .protocol_router()
        .with_v1(v1::agent(
            state.clone(),
            negotiation,
            owner.clone(),
            setup.elicitation.clone(),
        ))
        .with_v2(v2::agent(state, v2_negotiation, owner, setup.elicitation))
        .connect_to(transport)
        .await
}
