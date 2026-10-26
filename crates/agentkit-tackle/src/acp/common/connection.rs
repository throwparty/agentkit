//! Protocol-neutral connection setup shared by the transports.
//!
//! Both protocol versions are negotiated over the same connection, so the
//! MCP pool and the editor-toggle seed are built once, here, before the
//! version router runs. The transports see only this type; how a version
//! stores the state is its own business.

use crate::mcp::McpPool;
use crate::state::TackleState;

/// The version-neutral inputs a new connection needs.
pub(crate) struct ConnectionSetup {
    /// The `[client_tools]` seed for the fs surface.
    pub(crate) editor_fs: bool,
    /// The `[client_tools]` seed for the terminal surface.
    pub(crate) editor_terminal: bool,
    /// The connection's MCP pool: configured servers plus anything the
    /// client forwards later.
    pub(crate) pool: McpPool,
}

impl ConnectionSetup {
    /// Builds the connection's MCP pool and the editor seed from
    /// configuration.
    pub(crate) async fn load(state: &TackleState) -> Self {
        let client_tools = state.config().config.client_tools.clone().unwrap_or_default();
        Self {
            editor_fs: client_tools.fs_enabled,
            editor_terminal: client_tools.terminal_enabled,
            pool: connect_config_servers(state).await,
        }
    }
}

/// A connection's configured MCP servers, connected concurrently. Each
/// server connects on a throwaway pool, then the settled pools are
/// absorbed into one, so a slow server never blocks another.
async fn connect_config_servers(state: &TackleState) -> McpPool {
    let helper = crate::credentials::credential_helper_name(&state.config().config);
    let new_pool = || McpPool::new().with_credential_helper(helper.clone());
    let mut pool = new_pool();
    for (server_name, server_config) in &state.config().config.mcp_servers {
        pool.register(server_name, server_config.clone());
    }
    let connecting = state
        .config()
        .config
        .mcp_servers
        .iter()
        .map(|(server_name, server_config)| {
            let server_name = server_name.clone();
            let server_config = server_config.clone();
            let mut one = new_pool();
            one.register(&server_name, server_config.clone());
            async move {
                one.connect(&server_name, &server_config).await;
                one
            }
        })
        .collect::<Vec<_>>();
    for connected in futures::future::join_all(connecting).await {
        pool.absorb(connected);
    }
    pool
}
