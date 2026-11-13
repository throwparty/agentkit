//! The v2 notice sink: notices are capability-free, so a script-driven model
//! call always surfaces.

use super::negotiation::ConnectionNegotiation;
use crate::scripts::host::NoticeSink;
use crate::store::SessionId;
use agent_client_protocol::role::acp::Client;
use agent_client_protocol::v2::schema::{
    Notice, NoticeSeverity, SessionInfoUpdate, SessionUpdate, UpdateSessionNotification,
};
use agent_client_protocol::v2::ConnectionTo as V2ConnectionTo;
use std::sync::Arc;

pub(super) struct ConnectionNotices {
    pub cx: V2ConnectionTo<Client>,
    pub negotiation: Arc<ConnectionNegotiation>,
    pub source: &'static str,
}

impl ConnectionNotices {
    /// A sink over `cx` for `source`'s background work. The connection's state
    /// rides along so a session the client closed stays quiet: titling and
    /// compaction outlive the turn that started them.
    pub(super) fn new(
        cx: V2ConnectionTo<Client>,
        negotiation: Arc<ConnectionNegotiation>,
        source: &'static str,
    ) -> Self {
        Self {
            cx,
            negotiation,
            source,
        }
    }

    fn send(&self, session: &SessionId, update: SessionUpdate) {
        if self.negotiation.is_closed(session) {
            return;
        }
        let _ = self.cx.send_notification(UpdateSessionNotification::new(
            super::wire_session_id(session),
            update,
        ));
    }
}

impl NoticeSink for ConnectionNotices {
    fn script_model_call(&self, session: &SessionId) {
        self.send(
            session,
            SessionUpdate::Notice(Notice::new(
                NoticeSeverity::Info,
                format!("Running a background model call ({})", self.source),
            )),
        );
    }

    fn session_title(&self, session: &SessionId, title: &str) {
        self.send(
            session,
            SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().title(title.to_owned())),
        );
    }
}
