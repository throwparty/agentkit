//! The ACP server surface: version negotiation, the protocol-neutral common
//! layer, and one module per protocol version.
//!
//! `common` holds orchestration shared by every version. `v1` and `v2` hold
//! the per-version wire surfaces and must not depend on each other (enforced
//! by arch-lint). `transport` carries whichever version a connection
//! negotiates.

pub mod common;
pub mod http;
pub mod transport;
pub mod v1;
pub mod v2;

pub use transport::run_stdio;
pub use v1::{PendingProject, TackleState};

use agent_client_protocol::ConnectTo as _;
use common::connection::ConnectionSetup;
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
        .with_v1(v1::agent(state.clone(), negotiation, owner.clone()))
        .with_v2(v2::agent(state, v2_negotiation, owner))
        .connect_to(transport)
        .await
}
