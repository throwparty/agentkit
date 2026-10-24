//! Session replay: rendering the shared neutral replay stream as the v2
//! `session/update` notification sequence.
//!
//! v2 identity is carried per message (`messageId`), so replay uses whole-
//! message upserts rather than chunks: a retained message replays with the
//! same id it was stored under, and no chunk is ever sent during replay.

use crate::acp::common::replay::{walk, ReplayEvent};
use agent_client_protocol::schema::v2::{
    AgentMessage, ContentBlock, MessageId, SessionId, SessionUpdate, UpdateSessionNotification,
    UserMessage,
};

/// Maps stored messages to replay notifications via the shared neutral walk.
/// Tool calls are not replayed yet (they land with the tool-call surface).
pub(super) fn replay_updates(
    session_id: &SessionId,
    assembled: &[crate::store::AssembledMessage],
) -> Vec<UpdateSessionNotification> {
    walk(assembled)
        .into_iter()
        .flat_map(|event| render(session_id, event))
        .collect()
}

fn render(session_id: &SessionId, event: ReplayEvent) -> Vec<UpdateSessionNotification> {
    let ReplayEvent::Message {
        id,
        is_user,
        blocks_json,
    } = event
    else {
        return Vec::new();
    };
    let Ok(blocks) = serde_json::from_str::<Vec<ContentBlock>>(&blocks_json) else {
        return Vec::new();
    };
    let message_id = MessageId::new(id);
    let update = if is_user {
        SessionUpdate::UserMessage(UserMessage::new(message_id).content(blocks))
    } else {
        SessionUpdate::AgentMessage(AgentMessage::new(message_id).content(blocks))
    };
    vec![UpdateSessionNotification::new(
        session_id.clone(),
        update,
    )]
}
