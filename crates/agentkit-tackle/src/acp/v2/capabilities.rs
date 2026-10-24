//! The ACP v2 capability map: object-valued support markers grouped under
//! `session`, the required session baseline, the optional method markers, and
//! the draft fork marker.

use crate::state::TackleState;
use agent_client_protocol::schema::v2::{
    AgentCapabilities, McpCapabilities, McpHttpCapabilities, McpStdioCapabilities,
    PromptCapabilities, PromptEmbeddedContextCapabilities, PromptImageCapabilities,
    SessionAdditionalDirectoriesCapabilities, SessionCapabilities, SessionDeleteCapabilities,
    SessionForkCapabilities,
};

impl TackleState {
    /// The v2 capability map. Unlike v1, presence markers are objects and the
    /// session surface is nested under `session`.
    pub(super) fn v2_capabilities(&self) -> AgentCapabilities {
        AgentCapabilities::new().session(
            SessionCapabilities::new()
                .prompt(
                    PromptCapabilities::new()
                        .image(PromptImageCapabilities::default())
                        .embedded_context(PromptEmbeddedContextCapabilities::default()),
                )
                .mcp(
                    McpCapabilities::new()
                        .stdio(McpStdioCapabilities::default())
                        .http(McpHttpCapabilities::default()),
                )
                .delete(SessionDeleteCapabilities::default())
                .additional_directories(SessionAdditionalDirectoriesCapabilities::default())
                .fork(SessionForkCapabilities::default()),
        )
    }
}
