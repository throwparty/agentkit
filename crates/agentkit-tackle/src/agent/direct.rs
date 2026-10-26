//! Direct invocation: a user-typed `/!<tool> {json}` runs a client-capability
//! tool on the connection or an MCP tool against the pool; the result is
//! announced as a tool call and stored in the turn.

use crate::agent::runtime::TurnRuntime;
use crate::agent::sink::{ToolCallStatus, ToolCallUpdate};
use crate::state::{LeaseHeartbeat, TackleState};
use std::sync::Arc;

#[allow(clippy::too_many_arguments)] // the invocation's resolved inputs
pub(crate) async fn run_direct_invocation(
    state: Arc<TackleState>,
    runtime: TurnRuntime,
    turn_id: crate::store::TurnId,
    session_id: crate::store::SessionId,
    owner: Arc<String>,
    call_id: String,
    input: crate::invokables::DirectInvocation,
) -> Result<(), String> {
    let crate::agent::runtime::TurnRuntime {
        pool,
        sink,
        executor,
        tracker,
        client_tools,
        ..
    } = runtime;
    // The caller acquired the lease, but the invocation runs in this task,
    // so the heartbeat must live here for its whole duration.
    let _lease_heartbeat = LeaseHeartbeat::spawn(
        Arc::clone(&state.db),
        session_id.clone(),
        owner.as_ref().clone(),
    );
    let (text, is_error) = if input.invokable.starts_with("client.") {
        // Client-capability tools execute on the connection, not the MCP pool.
        match input.arguments_json() {
            Err(err) => (err.to_string(), true),
            Ok(_) if !client_tools.iter().any(|tool| tool.name == input.invokable) => (
                format!(
                    "client tool `{}` is not available on this connection",
                    input.invokable
                ),
                true,
            ),
            Ok(arguments) => match executor
                .execute(&session_id, &input.invokable, &arguments)
                .await
            {
                Ok(executed) => {
                    tracker.track(&session_id, executed.created, executed.finished);
                    (executed.content, false)
                }
                Err(err) => (err, true),
            },
        }
    } else {
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
    sink.tool_call_update(
        &session_id,
        ToolCallUpdate {
            id: call_id.clone(),
            status: if is_error {
                ToolCallStatus::Failed
            } else {
                ToolCallStatus::Completed
            },
            content: text.clone(),
        },
    );
    // Two stored messages per the schema: the call (namespaced invokable plus
    // arguments) and the result that answers it.
    let call = state
        .db
        .append_message(
            &turn_id,
            crate::store::Role::ToolCall,
            &input.arguments,
            Some(&input.invokable),
            None,
            None,
            Some(&call_id),
        )
        .await
        .map_err(|err| err.to_string())?;
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
        .map_err(|err| err.to_string())?;
    state.db.release_lease(&session_id, &owner).await.ok();
    Ok(())
}
