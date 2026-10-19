//! ACP server surface: protocol handling via the SDK's callback builders.
//!
//! Handlers are registered per request type and tried in order; shared
//! state rides in `Arc<TackleState>` captured by each callback, and
//! per-connection negotiation state in `Arc<ConnectionNegotiation>`. The
//! stdio transport is the normal launch mode (one process per client
//! connection); HTTP serves many connections per process.



mod capabilities;
mod commands;
mod notices;
mod permission;
mod server;
mod sink;
mod replay;
mod negotiation;
pub mod client_tools;
pub mod http;

pub use capabilities::UnstableFeature;
pub use negotiation::ConnectionNegotiation;
pub(crate) use commands::connect_config_servers;
pub(crate) use server::run_agent_over;
pub(crate) use sink::{ConnectionTurnSink, wire_kind};
pub use server::run_stdio;
pub use crate::state::{PendingProject, TackleState};
use crate::state::{LeaseHeartbeat, store_session_id};

use agent_client_protocol::schema::v1::{SessionId, SessionNotification, SessionUpdate};
use agent_client_protocol::Error;

use std::sync::Arc;








/// tool executes on the connection, an MCP tool against the pool; the result
/// is announced as a tool call and stored in the turn. Returns the response
/// the caller sends.
#[allow(clippy::too_many_arguments)] // the invocation's resolved inputs
pub(super) async fn run_direct_invocation(
    state: Arc<TackleState>,
    negotiation: Arc<ConnectionNegotiation>,
    cx: agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    wire_id: SessionId,
    turn_id: crate::store::TurnId,
    session_id: crate::store::SessionId,
    owner: Arc<String>,
    call_id: String,
    input: crate::invokables::DirectInvocation,
) -> Result<agent_client_protocol::schema::v1::PromptResponse, Error> {
    use agent_client_protocol::schema::v1::{ContentBlock, PromptResponse, StopReason};
    // The caller acquired the lease, but the invocation runs in this task,
    // so the heartbeat must live here for its whole duration.
    let _lease_heartbeat = LeaseHeartbeat::spawn(
        Arc::clone(&state.db),
        session_id.clone(),
        owner.as_ref().clone(),
    );
    let (text, is_error) = if input.invokable.starts_with("client.") {
        // Client-capability tools execute on the
        // connection, not the MCP pool.
        let available = crate::acp_v1::client_tools::catalog(
            negotiation.client_caps(),
            negotiation.editor_toggles(),
        );
        match input.arguments_json() {
            Err(err) => (err.to_string(), true),
            Ok(_) if !available.iter().any(|tool| tool.name == input.invokable) => (
                format!(
                    "client tool `{}` is not available on this connection",
                    input.invokable
                ),
                true,
            ),
            Ok(arguments) => {
                match crate::acp_v1::client_tools::execute(&cx, &wire_id, &input.invokable, &arguments)
                    .await
                {
                    Ok(executed) => {
                        negotiation.track_terminals(
                            &store_session_id(&wire_id),
                            executed.created,
                            executed.finished,
                        );
                        (executed.content, false)
                    }
                    Err(err) => (err, true),
                }
            }
        }
    } else {
        let pool = negotiation.mcp_pool();
        let pool = pool.lock().await;
        let tools = pool.list_tools().await;
        let executed = match crate::invokables::Registry::build(&state.definitions(), &tools) {
            Ok(registry) => registry
                .execute_direct(&pool, &input)
                .await
                .map_err(|err| err.to_string()),
            Err(err) => Err(err.to_string()),
        };
        drop(pool);
        match executed {
            Ok(result) => (result.content_json, result.is_error),
            Err(err) => (err, true),
        }
    };
    let status = if is_error {
        agent_client_protocol::schema::v1::ToolCallStatus::Failed
    } else {
        agent_client_protocol::schema::v1::ToolCallStatus::Completed
    };
    let _ = cx.send_notification(SessionNotification::new(
        wire_id.clone(),
        SessionUpdate::ToolCallUpdate(agent_client_protocol::schema::v1::ToolCallUpdate::new(
            agent_client_protocol::schema::v1::ToolCallId::new(call_id),
            agent_client_protocol::schema::v1::ToolCallUpdateFields::new()
                .status(status)
                .content(vec![
                    agent_client_protocol::schema::v1::ToolCallContent::from(ContentBlock::Text(
                        agent_client_protocol::schema::v1::TextContent::new(text.clone()),
                    )),
                ]),
        )),
    ));
    // Two stored messages per the schema: the call
    // (namespaced invokable plus arguments) and the
    // result that answers it.
    let call = state
        .db
        .append_message(
            &turn_id,
            crate::store::Role::ToolCall,
            &input.arguments,
            Some(&input.invokable),
            None,
            None,
            None,
        )
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?;
    state
        .db
        .append_message(
            &turn_id,
            crate::store::Role::ToolResult,
            &serde_json::json!([{ "type": "text", "text": text }]).to_string(),
            Some(&input.invokable),
            Some(&call.id),
            Some(is_error),
            None,
        )
        .await
        .map_err(|err| Error::internal_error().data(err.to_string()))?;
    state.db.release_lease(&session_id, &owner).await.ok();
    Ok(PromptResponse::new(StopReason::EndTurn))
}




#[cfg(test)]
mod tests;
