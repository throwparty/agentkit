//! ACP server surface: protocol handling via the SDK's callback builders.
//!
//! Handlers are registered per request type and tried in order; shared
//! state rides in `Arc<TackleState>` captured by each callback, and
//! per-connection negotiation state in `Arc<ConnectionNegotiation>`. The
//! stdio transport is the normal launch mode (one process per client
//! connection); HTTP arrives with T-036.

use crate::config::Loaded;
use crate::loader::Definitions;
use crate::store::SessionStore;
use agent_client_protocol::schema::v1::{
    AgentCapabilities, InitializeRequest, InitializeResponse, McpCapabilities, PromptCapabilities,
    SessionCapabilities, SessionCloseCapabilities, SessionDeleteCapabilities,
    SessionForkCapabilities, SessionListCapabilities, SessionResumeCapabilities,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{Agent, Stdio};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

/// RFD-shaped features the client advertised support for. Unstable
/// updates are sent only for features captured here; stable-variant
/// fallbacks cover everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UnstableFeature {
    Fork,
    SessionCompaction,
    SessionNotices,
}

impl UnstableFeature {
    /// The `_meta` key advertising this feature, per the extensibility
    /// conventions (feature names double as capability keys).
    pub fn meta_key(&self) -> &'static str {
        match self {
            UnstableFeature::Fork => "unstable_session_fork",
            UnstableFeature::SessionCompaction => "unstable_session_compaction",
            UnstableFeature::SessionNotices => "unstable_session_notices",
        }
    }

    /// The client-advertised subset of unstable features, read from
    /// `clientCapabilities._meta`.
    pub fn advertised_in(meta: &Option<agent_client_protocol::schema::v1::Meta>) -> BTreeSet<Self> {
        let mut supported = BTreeSet::new();
        for feature in [Self::Fork, Self::SessionCompaction, Self::SessionNotices] {
            if meta
                .as_ref()
                .is_some_and(|meta| meta.contains_key(feature.meta_key()))
            {
                supported.insert(feature);
            }
        }
        supported
    }
}

/// Per-connection negotiation state, captured at initialize and consulted
/// by every update sender.
#[derive(Default)]
pub struct ConnectionNegotiation {
    supported: Mutex<BTreeSet<UnstableFeature>>,
}

impl ConnectionNegotiation {
    fn capture(&self, client_meta: &Option<agent_client_protocol::schema::v1::Meta>) {
        *self.supported.lock().expect("negotiation poisoned") =
            UnstableFeature::advertised_in(client_meta);
    }

    /// Whether the client advertised support for `feature`.
    pub fn supports(&self, feature: UnstableFeature) -> bool {
        self.supported
            .lock()
            .expect("negotiation poisoned")
            .contains(&feature)
    }
}

/// State shared across one connection's handlers.
pub struct TackleState {
    pub db: SessionStore,
    pub config: Loaded,
    pub definitions: Definitions,
}

impl TackleState {
    /// The capability map: the stable v1 surface plus the unstable
    /// features tackle implements (advertised unconditionally — the gate
    /// applies to *sending* RFD-shaped updates, per FR-002).
    fn capabilities(&self) -> AgentCapabilities {
        let unstable: agent_client_protocol::schema::v1::Meta = [
            UnstableFeature::Fork,
            UnstableFeature::SessionCompaction,
            UnstableFeature::SessionNotices,
        ]
        .into_iter()
        .map(|feature| (feature.meta_key().to_owned(), serde_json::json!({})))
        .collect();
        AgentCapabilities::new()
            .load_session(true)
            .prompt_capabilities(PromptCapabilities::new().image(true).audio(false))
            .mcp_capabilities(McpCapabilities::new().http(true).sse(false))
            .session_capabilities(
                SessionCapabilities::new()
                    .list(Some(SessionListCapabilities::default()))
                    .close(Some(SessionCloseCapabilities::default()))
                    .resume(Some(SessionResumeCapabilities::default()))
                    .delete(Some(SessionDeleteCapabilities::default()))
                    .fork(Some(SessionForkCapabilities::default())),
            )
            .meta(unstable)
    }
}

/// Runs the ACP agent over stdio until the transport closes.
pub async fn run_stdio(state: Arc<TackleState>) -> agent_client_protocol::Result<()> {
    let negotiation = Arc::new(ConnectionNegotiation::default());
    let negotiation_for_init = negotiation.clone();
    let state_for_init = state.clone();

    Agent
        .builder()
        .name("tackle")
        .on_receive_request(
            async move |request: InitializeRequest, responder, _cx| {
                // Protocol version negotiation: tackle implements v1, so it
                // responds with v1 regardless of the client's latest; a
                // v1-only client proceeds, a v2-only client disconnects.
                negotiation_for_init.capture(&request.client_capabilities.meta);
                responder.respond(
                    InitializeResponse::new(ProtocolVersion::V1)
                        .agent_capabilities(state_for_init.capabilities()),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_to(Stdio::new())
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client_meta(pairs: &[&str]) -> Option<agent_client_protocol::schema::v1::Meta> {
        let meta: agent_client_protocol::schema::v1::Meta = pairs
            .iter()
            .map(|key| ((*key).to_owned(), serde_json::json!({})))
            .collect();
        Some(meta)
    }

    #[test]
    fn client_advertised_features_are_captured() {
        let meta = client_meta(&["unstable_session_fork", "unstable_session_notices"]);
        let supported = UnstableFeature::advertised_in(&meta);
        assert!(supported.contains(&UnstableFeature::Fork));
        assert!(supported.contains(&UnstableFeature::SessionNotices));
        assert!(!supported.contains(&UnstableFeature::SessionCompaction));
    }

    #[test]
    fn absent_meta_advertises_nothing() {
        assert!(UnstableFeature::advertised_in(&None).is_empty());
        assert!(UnstableFeature::advertised_in(&client_meta(&[])).is_empty());
    }
}
