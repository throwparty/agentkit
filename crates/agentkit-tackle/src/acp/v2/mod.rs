//! The ACP v2 server: the per-connection handler builder. v2 is a sibling of
//! `acp::v1`; both share orchestration through `acp::common` and neither may
//! import the other.

pub mod capabilities;
pub mod commands;
pub mod initialize;
pub mod mcp;
pub mod negotiation;
pub mod notices;
pub mod permission;
pub mod prompt;
pub mod replay;
pub mod server;
pub mod sessions;
pub mod sink;

pub(crate) use server::agent;

/// Sends one foreground `state_update` for a session.
pub(super) fn send_state(
    cx: &agent_client_protocol::V2ConnectionTo<agent_client_protocol::role::acp::Client>,
    session: &agent_client_protocol::schema::v2::SessionId,
    state: agent_client_protocol::schema::v2::StateUpdate,
) {
    let _ = cx.send_notification(
        agent_client_protocol::schema::v2::UpdateSessionNotification::new(
            session.clone(),
            agent_client_protocol::schema::v2::SessionUpdate::StateUpdate(state),
        ),
    );
}

/// The stored session id in v2 wire form.
pub(super) fn wire_session_id(
    id: &crate::store::SessionId,
) -> agent_client_protocol::schema::v2::SessionId {
    agent_client_protocol::schema::v2::SessionId::new(format!("sess_{id}"))
}

/// Strips the wire prefix back to the bare store id.
pub(super) fn store_session_id(
    id: &agent_client_protocol::schema::v2::SessionId,
) -> String {
    let raw = id.to_string();
    raw.strip_prefix("sess_").unwrap_or(&raw).to_owned()
}
