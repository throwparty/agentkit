//! v2 wire helpers: session-id mapping and foreground state emission.

use agent_client_protocol::schema::v2::{SessionId, SessionUpdate, StateUpdate, UpdateSessionNotification};
use agent_client_protocol::role::acp::Client;
use agent_client_protocol::V2ConnectionTo;

/// Sends one foreground `state_update` for a session.
pub(crate) fn send_state(
    cx: &V2ConnectionTo<Client>,
    session: &SessionId,
    state: StateUpdate,
) {
    let _ = cx.send_notification(UpdateSessionNotification::new(
        session.clone(),
        SessionUpdate::StateUpdate(state),
    ));
}

/// The stored session id in v2 wire form.
pub(crate) fn wire_session_id(id: &crate::store::SessionId) -> SessionId {
    SessionId::new(format!("sess_{id}"))
}

/// Strips the wire prefix back to the bare store id.
pub(crate) fn store_session_id(id: &SessionId) -> String {
    let raw = id.to_string();
    raw.strip_prefix("sess_").unwrap_or(&raw).to_owned()
}
