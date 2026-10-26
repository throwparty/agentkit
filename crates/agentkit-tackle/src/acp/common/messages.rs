//! Harness-authored message persistence shared by the version modules.

use super::content::{self, Block};
use crate::state::TackleState;
use crate::store::{Message, Role, TurnId};
use agent_client_protocol::Error;

/// Persists a harness-authored assistant text message on a turn. Returns the
/// stored message so the caller can reuse its id for the streaming chunk.
pub(crate) async fn append_assistant_text(
    state: &TackleState,
    turn_id: &TurnId,
    text: &str,
) -> Result<Message, Error> {
    state
        .db
        .append_message(
            turn_id,
            Role::Assistant,
            &content::encode(&[Block::text(text)]),
            None,
            None,
            None,
            None,
        )
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))
}
