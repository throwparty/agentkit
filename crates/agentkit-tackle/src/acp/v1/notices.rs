//! The notice sink for a live connection: a script-driven model call becomes a session notice when the client advertised the notices capability.

use super::capabilities::UnstableFeature;
use super::negotiation::ConnectionNegotiation;
use crate::state::wire_session_id;
use agent_client_protocol::schema::v1::{
    Notice, NoticeSeverity, SessionInfoUpdate, SessionNotification, SessionUpdate,
};

/// The notice sink for a live connection: a script-driven model call
/// becomes a session notice when the client advertised the notices
/// capability. A narrow seam — one-way notification only, never
/// the connection, so the script loop cannot issue client requests.
struct ConnectionNotices {
    cx: agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    enabled: bool,
    source: &'static str,
}

impl crate::scripts::host::NoticeSink for ConnectionNotices {
    fn script_model_call(&self, session: &crate::store::SessionId) {
        if !self.enabled {
            return;
        }
        let notice = Notice::new(
            NoticeSeverity::Info,
            format!("Running a background model call ({})", self.source),
        );
        let _ = self.cx.send_notification(SessionNotification::new(
            wire_session_id(session),
            SessionUpdate::Notice(notice),
        ));
    }

    fn session_title(&self, session: &crate::store::SessionId, title: &str) {
        let _ = self.cx.send_notification(SessionNotification::new(
            wire_session_id(session),
            SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().title(title.to_owned())),
        ));
    }
}

/// Builds the notice sink for `cx`, gated on the client's advertised
/// notices capability and labelled with the calling path.
pub(super) fn connection_notices(
    cx: &agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    negotiation: &ConnectionNegotiation,
    source: &'static str,
) -> std::sync::Arc<dyn crate::scripts::host::NoticeSink> {
    std::sync::Arc::new(ConnectionNotices {
        cx: cx.clone(),
        enabled: negotiation.supports(UnstableFeature::SessionNotices),
        source,
    })
}
