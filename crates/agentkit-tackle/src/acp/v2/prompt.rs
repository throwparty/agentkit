//! The v2 `session/prompt` handler: insert the user message and respond with
//! its id immediately, then drive the turn and the foreground state machine on
//! a spawned task.

use super::negotiation::ConnectionNegotiation;
use crate::acp::common::session::{begin_prompt_turn, BeginError};
use crate::agent::config_options::session_actor;
use crate::agent::prompt::run_prompt_turn;
use crate::agent::runtime::TurnRuntime;
use crate::agent::TurnStop;
use crate::invokables::client_tools::{ClientToolExecutor, Executed};
use std::future::Future;
use std::pin::Pin;
use crate::scripts::host::NoticeSink;
use crate::state::TackleState;
use crate::store::SessionId;
use agent_client_protocol::role::acp::Client;
use agent_client_protocol::schema::v2::{
    ContentBlock, ErrorStopReason, IdleStateUpdate, PromptRequest, PromptResponse,
    RunningStateUpdate, SessionUpdate, StateUpdate, StopReason, UpdateSessionNotification,
    UserMessage,
};
use agent_client_protocol::{Error, Responder, V2ConnectionTo};
use std::sync::Arc;

/// `session/prompt`: the insertion response is the user message id; the turn
/// runs detached, reporting `running` then a terminal `idle`.
pub(super) async fn handle_prompt(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    cx: V2ConnectionTo<Client>,
    owner: Arc<String>,
    request: PromptRequest,
    responder: Responder<PromptResponse>,
) -> Result<(), Error> {
    let session_id = super::store_session_id(&request.session_id);
    let prompt_blocks = request.prompt.clone();
    let blocks_json =
        serde_json::to_string(&request.prompt).unwrap_or_else(|_| "[]".to_owned());
    let first_text = request
        .prompt
        .iter()
        .find_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.trim().to_owned()),
            _ => None,
        })
        .unwrap_or_default();

    let begun = match begin_prompt_turn(&state, session_id.clone(), blocks_json, owner.as_str())
        .await
    {
        Ok(begun) => begun,
        Err(BeginError::Owned { holder }) => {
            return responder.respond_with_error(Error::internal_error().data(
                serde_json::json!({
                    "error": "session-actively-owned",
                    "owner": holder,
                }),
            ));
        }
        Err(BeginError::Store(err)) => {
            return responder.respond_with_error(Error::internal_error().data(err));
        }
    };
    let _heartbeat = begun.heartbeat;
    let user_message_id = begun.user_message_id;
    let turn = begun.turn;
    let store_session = begun.store_session;
    let blocks_json = begun.blocks_json;

    let session_row = state.db.get_session(&store_session).await.ok().flatten();
    let actor_name = session_row
        .as_ref()
        .map(|row| session_actor(row, state.config().config.defaults.actor.as_deref()))
        .unwrap_or_else(|| "default".into());

    let request_cancellation = responder.cancellation();
    let (cancellation, registration) = negotiation.register_cancellation(&store_session);
    {
        let cancellation = cancellation.clone();
        cx.spawn(async move {
            request_cancellation.cancelled().await;
            cancellation.cancel();
            Ok(())
        })?;
    }

    // The acceptance receipt: the persisted user message id, sent at
    // insertion time rather than at turn end.
    responder.respond(PromptResponse::new(user_message_id.clone()))?;
    cx.send_notification(UpdateSessionNotification::new(
        request.session_id.clone(),
        SessionUpdate::UserMessage(
            UserMessage::new(agent_client_protocol::schema::v2::MessageId::new(user_message_id))
                .content(prompt_blocks),
        ),
    ))
    .map_err(|err| Error::internal_error().data(err.to_string()))?;

    let wire_session = super::wire_session_id(&store_session);
    let runtime = TurnRuntime {
        cancellation: cancellation.clone(),
        pool: negotiation.mcp_pool(),
        sink: Arc::new(super::sink::ConnectionTurnSink::new(cx.clone())),
        requester: Arc::new(super::permission::AcpPermissionRequester {
            cx: cx.clone(),
            session_id: wire_session.clone(),
        }),
        executor: Arc::new(NoClientTools),
        tracker: negotiation.clone(),
        compaction_notices: Arc::new(NullNotices),
        titling_notices: Arc::new(NullNotices),
        client_tools: Vec::new(),
    };

    let turn_cx = cx.clone();
    cx.spawn(async move {
        let _registration = registration;
        super::send_state(
            &turn_cx,
            &wire_session,
            StateUpdate::Running(RunningStateUpdate::new()),
        );
        let outcome = run_prompt_turn(
            state,
            runtime,
            store_session,
            turn,
            actor_name,
            session_row,
            first_text,
            blocks_json,
            owner,
        )
        .await;
        let stop = match outcome {
            Ok(TurnStop::EndTurn | TurnStop::ContextLength) => StopReason::EndTurn,
            Ok(TurnStop::MaxTurnRequests) => StopReason::MaxTurnRequests,
            Ok(TurnStop::Cancelled) => StopReason::Cancelled,
            Err(_) if cancellation.is_cancelled() => StopReason::Cancelled,
            Err(_) => StopReason::Error(ErrorStopReason::new()),
        };
        super::send_state(
            &turn_cx,
            &wire_session,
            StateUpdate::Idle(IdleStateUpdate::new().stop_reason(Some(stop))),
        );
        Ok(())
    })
}

/// ACP v2 removed the client filesystem and terminal surface, so the catalog
/// is empty and this is never reached.
struct NoClientTools;

impl ClientToolExecutor for NoClientTools {
    fn execute(
        &self,
        _session: &SessionId,
        _invokable: &str,
        _arguments: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<Executed, String>> + Send + '_>> {
        Box::pin(async { Err("no client tools under ACP v2".to_owned()) })
    }
}

struct NullNotices;

impl NoticeSink for NullNotices {
    fn script_model_call(&self, _session: &SessionId) {}
}
