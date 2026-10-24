//! The v2 notice sink: notices are capability-free, so a script-driven model
//! call always surfaces.

use crate::scripts::host::NoticeSink;
use crate::store::SessionId;
use agent_client_protocol::role::acp::Client;
use agent_client_protocol::schema::v2::{
    Notice, NoticeSeverity, SessionUpdate, UpdateSessionNotification,
};
use agent_client_protocol::V2ConnectionTo;

pub(super) struct ConnectionNotices {
    pub cx: V2ConnectionTo<Client>,
    pub source: &'static str,
}

impl NoticeSink for ConnectionNotices {
    fn script_model_call(&self, session: &SessionId) {
        let _ = self.cx.send_notification(UpdateSessionNotification::new(
            super::wire_session_id(session),
            SessionUpdate::Notice(Notice::new(
                NoticeSeverity::Info,
                format!("Running a background model call ({})", self.source),
            )),
        ));
    }
}
