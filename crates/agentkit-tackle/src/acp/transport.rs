//! Transport entry points shared by every protocol version.
//!
//! The stdio launch lives here rather than in a version module because the
//! transport is version-agnostic: it hands the connection to the negotiated
//! implementation and carries whichever version results.

use crate::acp::v1::{
    client_tools, connect_config_servers, run_agent_over, ConnectionNegotiation, TackleState,
};
use agent_client_protocol::Stdio;
use std::sync::Arc;

/// Launches the ACP agent over stdio, one connection per process. Ctrl-C and
/// SIGTERM end the transport cleanly.
pub async fn run_stdio(state: Arc<TackleState>) -> agent_client_protocol::Result<()> {
    let editor = client_tools::EditorToggles::from_config(&state.config().config);
    let pool = connect_config_servers(&state).await;
    let negotiation = Arc::new(ConnectionNegotiation::new(editor, pool));
    tokio::select! {
        result = run_agent_over(state, negotiation, Stdio::new()) => result,
        _ = crate::cli::wait_for_shutdown() => Ok(()),
    }
}
